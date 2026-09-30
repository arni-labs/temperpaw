//! Iterative feedback is exercised through actual guests with deterministic provider replies.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn bytes(module: &str) -> Vec<u8> {
    if module == "semantic_call"
        && let Ok(path) = std::env::var("ARN518_CALL_WASM_OVERRIDE")
    {
        return std::fs::read(path).unwrap();
    }
    if module == "semantic_step"
        && let Ok(path) = std::env::var("ARN518_STEP_WASM_OVERRIDE")
    {
        return std::fs::read(path).unwrap();
    }
    if module == "semantic_expand"
        && let Ok(path) = std::env::var("ARN518_OUTLOOK_EXPAND_WASM_OVERRIDE")
    {
        return std::fs::read(path).unwrap();
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output=std::process::Command::new("bash").current_dir(&root).args(["-c",&format!("set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/{module} wasm32-unknown-unknown --locked")]).output().expect("build semantic WASM");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
}
async fn invoke(engine: &WasmEngine, module: &str, fields: Value) -> Value {
    let hash = engine.compile_and_cache(&bytes(module)).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "run-fixture".into(),
        trigger_action: "Next".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"].as_u64().unwrap_or(0)},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::new(),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    invoke_with_host(
        engine,
        module,
        ctx,
        Arc::new(SimWasmHost::new().with_default_response(500, "unexpected provider IO")),
        hash,
    )
    .await
}
async fn invoke_with_host(
    engine: &WasmEngine,
    _module: &str,
    ctx: WasmInvocationContext,
    host: Arc<dyn WasmHost>,
    hash: String,
) -> Value {
    let r = engine
        .invoke(
            &hash,
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
    serde_json::to_value(r).unwrap()
}

#[derive(Default)]
struct WorldProvider {
    requests: Mutex<Vec<Value>>,
    fail: AtomicBool,
    drift: AtomicBool,
    slices: AtomicBool,
}
#[async_trait::async_trait]
impl WasmHost for WorldProvider {
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        _headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        assert_eq!(method, "POST");
        assert_eq!(url, "https://api.typesafe.ai/v1/systemone");
        let request: Value = serde_json::from_str(body).unwrap();
        self.requests.lock().unwrap().push(request.clone());
        if self.fail.load(Ordering::SeqCst) {
            return Ok((503, "fixture transient failure".into()));
        }
        let probability = if self.drift.load(Ordering::SeqCst) {
            0.23 + 0.1 * ((self.requests.lock().unwrap().len() - 1) / 4) as f64
        } else {
            0.23
        };
        let answers: serde_json::Map<String, Value> = request["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, q)| {
                let answer = if q["type"] == "noul" {
                    json!({"type":"noul","noul":probability})
                } else {
                    let options = q["criteria"].as_object().unwrap();
                    let choice = if options.contains_key("alternative_answers") {
                        if self.slices.load(Ordering::SeqCst) {
                            "complementary_slices"
                        } else {
                            "alternative_answers"
                        }
                    } else if options.contains_key("compatible") {
                        "compatible"
                    } else if options.contains_key("plausible") {
                        "plausible"
                    } else {
                        "none"
                    };
                    let probabilities: serde_json::Map<String, Value> = options
                        .keys()
                        .map(|k| (k.clone(), json!(if k == choice { 1.0 } else { 0.0 })))
                        .collect();
                    json!({"type":"choice","choice":choice,"probabilities":probabilities})
                };
                (key.clone(), answer)
            })
            .collect();
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }

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
}
fn snapshot() -> Value {
    let mut s = json!({"world":{"Id":"question","target_date":"2027-09-20","last_ingest_date":"2026-09-20","hindcast_mode":"false"},"nodes":[
      {"Id":"e","kind":"evidence","statement":"A clinic already uses an assistant for appointment reminders.","quote":"The assistant sends appointment reminders today.","observed_at":"2026-09-19","claim_type":"observed","source_refs":["https://example.org/clinic"],"edges":"[]"},
      {"Id":"h1","kind":"scenario","statement":"Clinics automate bookings by September 2027.","edges":"[]"},
      {"Id":"h2","kind":"revision","statement":"Patients accept assistant-run bookings by September 2027.","edges":"[]"},
      {"Id":"counter","kind":"scenario","statement":"Patients demand human approval by September 2027.","edges":"[]"},
      {"Id":"w1","kind":"world","statement":"By September 2027 clinics run bookings without reception staff and patients accept this change.","component_ids":["h1","h2"],"counter_ids":["counter"],"resolve_by":"2027-09-20","edges":"[{\"kind\":\"requires\",\"to_id\":\"h1\"},{\"kind\":\"requires\",\"to_id\":\"h2\"},{\"kind\":\"requires\",\"to_id\":\"e\"}]"},
      {"Id":"w2","kind":"world","statement":"By September 2027 clinics automate routine bookings while humans approve every change.","component_ids":["h1","h2"],"counter_ids":["counter"],"resolve_by":"2027-09-20","edges":"[]"}
    ]});
    s["nodes"].as_array_mut().unwrap().push(json!({"Id":"h3","kind":"scenario","statement":"Clinics reduce reception staffing by September 2027.","edges":"[]"}));
    for i in [4, 5] {
        s["nodes"][i]["component_ids"] = json!(["h1", "h2", "h3"]);
        s["nodes"][i]["facets"] = json!([{"id":"f1","title":"Bookings","description":"Booking work changes","component_ids":["h1"]},{"id":"f2","title":"Patients","description":"Patients accept the change","component_ids":["h2"]},{"id":"f3","title":"Staff","description":"Staffing changes","component_ids":["h3"]}]);
        s["nodes"][i]["assumptions"] = json!([]);
        s["nodes"][i]["chain"] = json!([{"id":"a","from_ids":["h1"],"to_id":"h2","mechanism":"Reliable automation earns acceptance","by":"2027-03-01"},{"id":"b","from_ids":["h2"],"to_id":"h3","mechanism":"Acceptance permits fewer reception shifts","by":"2027-09-20"}]);
    }
    s
}
fn answer(snapshot: &Value) -> Value {
    let outcome = |index: usize| {
        let n = &snapshot["nodes"][index];
        json!({"id":n["Id"],"world_id":n["Id"],"title":"A different day at the clinic","definition":n["statement"],"component_ids":n["component_ids"],"counter_ids":n["counter_ids"],"scenario_ids":["h1","h2","e"],"probability":0.99,"narrative":"Bookings change who spends time on the phone.","scene":"A receptionist helps a worried patient while bookings arrive automatically.","what_you_can_do":["Ask a clinic how exceptions are handled."],"signals":["Fewer calls to reception"],"falsifiers":["Patients insist on calling"]})
    };
    json!({"schema":"foresight-worlds-v3","probability_model":"overlapping_worlds","probability_basis":"model_implied_world_estimate","calibrated":false,"evaluation_status":"evaluated","evaluation_note":"","headline":"Two possible clinic mornings","summary":"Both worlds build on reminders that already exist.","horizon":"2027-09-20","baseline":{"as_of":"2026-09-20","observed":[{"claim":"Appointment reminders already run automatically.","evidence_ids":["e"]}],"assumptions":[],"unknowns":["Will patients accept unsupervised changes?"]},"evidence_limits":["One observed clinic; model estimates are uncalibrated."],"research_questions":[],"outcomes":[outcome(4),outcome(5)]})
}

async fn prepared(engine: &WasmEngine) -> Value {
    let full = snapshot();
    let mut s = full.clone();
    s["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["kind"] != "world");
    let worlds: Vec<Value> = [4usize, 5]
        .iter()
        .map(|&i| {
            let mut w = full["nodes"][i].clone();
            w["id"] = w["Id"].clone();
            for (key, text) in [
                ("title", "A new clinic morning"),
                (
                    "mechanism",
                    "Bookings, acceptance and staffing change together",
                ),
                ("scene", "A clinic opens without booking calls waiting"),
                ("narrative", "Staff help patients rather than arrange times"),
            ] {
                w[key] = json!(text);
            }
            for key in ["signals", "falsifiers", "what_you_can_do"] {
                w[key] = json!(["Observe reception staffing"]);
            }
            w
        })
        .collect();
    let result = json!({"baseline":answer(&full)["baseline"],"worlds":worlds});
    let fields = json!({"phase":"compose","snapshot_json":s.to_string(),"program_json":json!({"round":1,"tasks":[],"cursor":0,"results":{},"evaluations":{}}).to_string(),"reasoning_result":result.to_string()});
    let r = invoke(engine, "semantic_expand", fields).await;
    assert_eq!(r["callback_action"], "Expanded", "{r}");
    let mut fields = r["callback_params"].clone();
    fields["trace_json"] = json!("[]");
    fields["phase"] = json!("compose");
    fields["started_at_ms"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string()
    );
    fields
}
async fn call(engine: &WasmEngine, fields: Value, host: Arc<WorldProvider>) -> Value {
    let hash = engine.compile_and_cache(&bytes("semantic_call")).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "feedback-test".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("semantic_call".into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"].as_u64().unwrap_or(0)},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-only".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    invoke_with_host(engine, "semantic_call", ctx, host, hash).await
}
async fn complete_pass(engine: &WasmEngine, mut fields: Value, host: Arc<WorldProvider>) -> Value {
    for _ in 0..100 {
        let result = call(engine, fields.clone(), host.clone()).await;
        assert_eq!(result["callback_action"], "Recorded");
        apply(&mut fields, &result);
        let mut p = program(&fields);
        if p["tasks"][0]["function"] == "check_world_set" && p["cursor"] == 1 {
            let audit = invoke(engine, "semantic_step", fields.clone()).await;
            assert_eq!(audit["callback_action"], "SearchPlanned");
            apply(&mut fields, &audit);
            p = program(&fields);
        }
        if p["cursor"].as_u64().unwrap() as usize >= p["tasks"].as_array().unwrap().len()
            || p["stop_reason"] == "provider_error"
        {
            return result;
        }
    }
    panic!("pass did not complete within fixture bound")
}
fn apply(fields: &mut Value, callback: &Value) {
    for (k, v) in callback["callback_params"].as_object().unwrap() {
        fields[k] = v.clone();
    }
}
fn program(fields: &Value) -> Value {
    serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn prior_judgments_reenter_requests_without_reusing_cached_answers() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    let mut fields = prepared(&engine).await;
    let first = complete_pass(&engine, fields.clone(), host.clone()).await;
    assert_eq!(first["callback_action"], "Recorded", "{first}");
    apply(&mut fields, &first);
    let first_trace: Value = serde_json::from_str(fields["trace_json"].as_str().unwrap()).unwrap();
    let first_http = host.requests.lock().unwrap().len();
    assert!(first_http > 0);
    let next = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(
        next["callback_action"], "SearchPlanned",
        "callback={}",
        next["callback_action"]
    );
    apply(&mut fields, &next);
    let p = program(&fields);
    assert_eq!(p["world_pass"], 2);
    let history = p["world_refinement"].clone();
    for id in p["active_world_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        assert_eq!(history[id]["rounds"].as_array().unwrap().len(), 1);
        assert_eq!(history[id]["rounds"][0]["probability"], 0.23);
        assert!(p["results"][id]["estimate_likelihood"].is_null());
    }
    let second = complete_pass(&engine, fields.clone(), host.clone()).await;
    assert_eq!(second["callback_action"], "Recorded", "{second}");
    apply(&mut fields, &second);
    let trace: Value = serde_json::from_str(fields["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        &trace.as_array().unwrap()[..first_trace.as_array().unwrap().len()],
        first_trace.as_array().unwrap()
    );
    assert_eq!(
        trace.as_array().unwrap().len(),
        first_trace.as_array().unwrap().len() * 2 - 1
    );
    {
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests.len(), first_http * 2 - 1);
        let second_input = requests[first_http].to_string();
        assert!(second_input.contains("previous_world_judgments"));
        assert!(second_input.contains("0.23"));
        assert!(second_input.contains("check_world_consistency"));
        let feedback = &requests[first_http]["state"]["common"]["previous_world_judgments"];
        let legend = feedback["question_legend"].as_array().unwrap();
        let values = feedback["rounds"][0]["judgments"].as_array().unwrap();
        assert_eq!(legend.len(), values.len());
        for (question, value) in legend.iter().zip(values) {
            match question["function"].as_str().unwrap() {
                "estimate_likelihood" | "conditional_on" | "conditional_off" => {
                    assert_eq!(value, "0.23")
                }
                "check_transition" => assert_eq!(value, "plausible"),
                _ => assert_eq!(value, "compatible"),
            }
        }
    }
    let finished = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(finished["callback_action"], "Reason", "{finished}");
    assert_eq!(finished["callback_params"]["phase"], "synthesize");
    apply(&mut fields, &finished);
    let p = program(&fields);
    for id in p["active_world_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        let receipt = &p["world_refinement"][id];
        assert_eq!(receipt["rounds"][0], history[id]["rounds"][0]);
        assert_eq!(receipt["rounds"].as_array().unwrap().len(), 2);
        assert_eq!(receipt["converged"], true);
        assert_eq!(receipt["accuracy_verified"], false);
    }
    let current: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let mut output = answer(&snapshot());
    let worlds: Vec<_> = current["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "world" && n["archived"] != true)
        .collect();
    for (outcome, world) in output["outcomes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(worlds)
    {
        outcome["id"] = world["Id"].clone();
        outcome["world_id"] = world["Id"].clone();
        outcome["definition"] = world["statement"].clone();
        outcome["component_ids"] = world["component_ids"].clone();
        outcome["counter_ids"] = world["counter_ids"].clone();
    }
    fields["phase"] = json!("synthesize");
    fields["reasoning_result"] = json!(output.to_string());
    let completed = invoke(&engine, "semantic_expand", fields).await;
    assert_eq!(
        completed["callback_action"], "Complete",
        "{}",
        completed["callback_params"]["error_message"]
    );
    let output: Value =
        serde_json::from_str(completed["callback_params"]["answer"].as_str().unwrap()).unwrap();
    assert_eq!(output["world_set_audit"], p["world_set_audit"]);
    assert!(
        output["evaluation_note"]
            .as_str()
            .unwrap()
            .contains("Jev judged")
    );
    for outcome in output["outcomes"].as_array().unwrap() {
        assert_eq!(
            outcome["refinement"],
            p["world_refinement"][outcome["world_id"].as_str().unwrap()]
        );
        assert_eq!(outcome["probability"], 0.23);
    }
}

#[tokio::test]
async fn provider_failure_does_not_claim_convergence_or_discard_prior_pass() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    let mut fields = prepared(&engine).await;
    let first = complete_pass(&engine, fields.clone(), host.clone()).await;
    apply(&mut fields, &first);
    let next = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(next["callback_action"], "SearchPlanned");
    apply(&mut fields, &next);
    let history = program(&fields)["world_refinement"].clone();
    host.fail.store(true, Ordering::SeqCst);
    let failed = complete_pass(&engine, fields.clone(), host.clone()).await;
    assert_eq!(failed["callback_action"], "Recorded");
    apply(&mut fields, &failed);
    assert_eq!(program(&fields)["stop_reason"], "provider_error");
    let stopped = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_ne!(stopped["callback_action"], "Complete");
    assert_ne!(stopped["callback_action"], "SearchPlanned");
    apply(&mut fields, &stopped);
    let p = program(&fields);
    for id in p["active_world_ids"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        assert_eq!(
            p["world_refinement"][id]["rounds"][0],
            history[id]["rounds"][0]
        );
        assert_ne!(p["world_refinement"][id]["converged"], true);
        assert_eq!(p["world_refinement"][id]["accuracy_verified"], false);
    }
}

#[tokio::test]
async fn changing_estimates_stop_at_three_passes_and_expired_budget_spends_no_http() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    host.drift.store(true, Ordering::SeqCst);
    let mut fields = prepared(&engine).await;
    for pass in 1..=3 {
        let r = complete_pass(&engine, fields.clone(), host.clone()).await;
        assert_eq!(r["callback_action"], "Recorded");
        apply(&mut fields, &r);
        let next = invoke(&engine, "semantic_step", fields.clone()).await;
        if pass < 3 {
            assert_eq!(
                next["callback_action"], "SearchPlanned",
                "pass{pass}: {next}"
            );
        } else {
            assert_eq!(next["callback_action"], "Reason");
            assert_eq!(next["callback_params"]["phase"], "synthesize");
        }
        apply(&mut fields, &next);
    }
    let p = program(&fields);
    for id in p["active_world_ids"].as_array().unwrap() {
        let r = &p["world_refinement"][id.as_str().unwrap()];
        assert_eq!(r["rounds"].as_array().unwrap().len(), 3);
        assert_eq!(r["converged"], false);
        assert_eq!(r["accuracy_verified"], false);
    }
    let empty_host = Arc::new(WorldProvider::default());
    let mut expired = prepared(&engine).await;
    expired["started_at_ms"] = json!(
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            - 3_600_001)
            .to_string()
    );
    let r = call(&engine, expired, empty_host.clone()).await;
    assert_eq!(r["callback_action"], "Recorded");
    assert_eq!(program(&r["callback_params"])["stop_reason"], "time_budget");
    assert_eq!(empty_host.requests.lock().unwrap().len(), 0);
    let mut exhausted = prepared(&engine).await;
    exhausted["trace_json"] =
        json!(json!(vec![json!({"fixture":"already-counted"}); 5000]).to_string());
    let r = call(&engine, exhausted, empty_host.clone()).await;
    assert_eq!(r["callback_action"], "Recorded");
    assert_eq!(program(&r["callback_params"])["stop_reason"], "call_budget");
    assert_eq!(empty_host.requests.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn optional_refinement_declines_before_erasing_completed_world_odds() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    let mut fields = prepared(&engine).await;
    loop {
        let p = program(&fields);
        if p["cursor"].as_u64().unwrap() as usize >= p["tasks"].as_array().unwrap().len() {
            break;
        }
        let result = complete_pass(&engine, fields.clone(), host.clone()).await;
        assert_eq!(result["callback_action"], "Recorded");
        apply(&mut fields, &result);
    }
    let before = program(&fields);
    fields["transition_count"] = json!(410);
    let stopped = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(stopped["callback_action"], "Reason", "{stopped}");
    assert_eq!(stopped["callback_params"]["phase"], "synthesize");
    let after: Value =
        serde_json::from_str(stopped["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["results"], before["results"]);
    assert_eq!(after["evaluations"], before["evaluations"]);
    assert_eq!(after["world_pass"], before["world_pass"]);
    assert_eq!(after["refinement_admission"]["admitted"], false);
    for id in after["active_world_ids"].as_array().unwrap() {
        let r = &after["world_refinement"][id.as_str().unwrap()];
        assert_eq!(r["stop_reason"], "transition_budget");
        assert_eq!(r["rounds"][0]["probability"], 0.23);
        assert_eq!(r["rounds"][0]["complete"], true);
    }
    let mut conflict_fields = fields.clone();
    let mut conflict_program = before.clone();
    for id in before["active_world_ids"].as_array().unwrap() {
        conflict_program["results"][id.as_str().unwrap()]["check_world_consistency"] =
            json!("conflict");
    }
    conflict_program["world_pass"] = json!(3);
    conflict_program["refinement_admission"] = json!({"admitted":true});
    conflict_fields["program_json"] = json!(conflict_program.to_string());
    let conflict = invoke(&engine, "semantic_step", conflict_fields).await;
    assert_eq!(conflict["callback_action"], "Reason");
    assert_eq!(conflict["callback_params"]["phase"], "synthesize");
    let conflict_result: Value = serde_json::from_str(
        conflict["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    for id in before["active_world_ids"].as_array().unwrap() {
        assert_eq!(
            conflict_result["world_audits"][id.as_str().unwrap()]["status"],
            "conflicts_found"
        );
    }
    fields["transition_count"] = json!(100);
    let admitted = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(admitted["callback_action"], "SearchPlanned", "{admitted}");
    let p: Value = serde_json::from_str(
        admitted["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(p["refinement_admission"]["admitted"], true);
    assert_eq!(p["world_pass"], 2);
}

#[tokio::test]
#[ignore = "Requires prepared native UUID packing fixture"]
async fn captured_uuid_deep_batch_preserves_every_comparison() {
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("FORESIGHT_PACKING_OUTPUT").unwrap()).unwrap(),
    )
    .unwrap();
    let mut fields = json!({"snapshot_json":raw["snapshot_json"],"program_json":raw["program_json"],"trace_json":"[]"});
    fields["started_at_ms"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string()
    );
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    let result = call(&engine, fields, host.clone()).await;
    assert_eq!(result["callback_action"], "Recorded");
    let requests = host.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request["questions"].as_object().unwrap().len(),
        raw["expected_tasks"].as_u64().unwrap() as usize
    );
    assert!(request["questions"].as_object().unwrap().len() > 1);
    assert!(request.to_string().len() <= 51928);
    for (i, original) in raw["individual"].as_array().unwrap().iter().enumerate() {
        let mut state = request["state"]["cases"][format!("q{i}")].clone();
        for (k, v) in request["state"]["common"].as_object().unwrap() {
            state[k] = v.clone();
        }
        if let Some(refs) = state.as_object_mut().unwrap().remove("comparison_refs") {
            state["comparisons"] = json!(
                refs.as_array()
                    .unwrap()
                    .iter()
                    .map(
                        |r| request["state"]["comparison_catalog"][r.as_u64().unwrap() as usize]
                            .clone()
                    )
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(state, original["state"]);
    }
}

#[tokio::test]
async fn world_set_review_precedes_world_checks_and_preserves_rejected_revision() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    host.slices.store(true, Ordering::SeqCst);
    let mut fields = prepared(&engine).await;
    let original = program(&fields);
    assert_eq!(original["tasks"][0]["function"], "check_world_set");
    let recorded = call(&engine, fields.clone(), host.clone()).await;
    assert_eq!(recorded["callback_action"], "Recorded");
    apply(&mut fields, &recorded);
    let p = program(&fields);
    assert_eq!(p["cursor"], 1);
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    assert_eq!(
        host.requests.lock().unwrap()[0]["questions"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    let request = host.requests.lock().unwrap()[0].clone();
    assert_eq!(
        request["state"]["cases"]["q0"]["proposed_worlds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fields["transition_count"] = json!(245);
    let revised = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(revised["callback_action"], "Reason");
    assert_eq!(revised["callback_params"]["phase"], "compose");
    let revised_p: Value =
        serde_json::from_str(revised["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        revised_p["world_set_audit"]["verdict"],
        "complementary_slices"
    );
    assert_eq!(revised_p["active_world_ids"], original["active_world_ids"]);
    assert_eq!(revised_p["results"], p["results"]);
    assert!(revised["callback_params"].get("snapshot_json").is_none());
    fields["transition_count"] = json!(410);
    let bounded = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(bounded["callback_action"], "SearchPlanned");
    apply(&mut fields, &bounded);
    assert_eq!(
        program(&fields)["world_set_audit"]["correction_status"],
        "transition_budget"
    );
    let next = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(next["callback_action"], "Evaluate");
    assert_eq!(host.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "Requires captured final world snapshot/program"]
async fn captured_world_set_correction_fits_first_draft_budget() {
    let raw: Value = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("FORESIGHT_WORLD_SET_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let f = raw.get("fields").unwrap_or(&raw);
    let mut p: Value = serde_json::from_str(f["program_json"].as_str().unwrap()).unwrap();
    let ids = p["active_world_ids"].as_array().unwrap().clone();
    let key = ids
        .iter()
        .map(|id| {
            let id = id.as_str().unwrap();
            format!("{}:{id}", id.len())
        })
        .collect::<Vec<_>>()
        .join(":");
    let task = json!({"nodeId":format!("world-set:{key}"),"function":"check_world_set","world_ids":ids,"depth":0});
    for t in p["tasks"].as_array().unwrap().clone() {
        for collection in ["results", "evaluations"] {
            if let Some(values) = p[collection][t["nodeId"].as_str().unwrap()].as_object_mut() {
                values.remove(t["function"].as_str().unwrap());
            }
        }
    }
    p["tasks"].as_array_mut().unwrap().insert(0, task.clone());
    p["cursor"] = json!(1);
    p["world_pass"] = json!(1);
    p["world_revision"] = json!(1);
    p["world_refinement"] = json!({});
    p["world_set_audit"] = Value::Null;
    p["stop_reason"] = json!("round_evaluated");
    p["results"][task["nodeId"].as_str().unwrap()]["check_world_set"] =
        json!("complementary_slices");
    p["evaluations"][task["nodeId"].as_str().unwrap()]["check_world_set"] = json!({"type":"choice","selected":"complementary_slices","answer":{"type":"choice","choice":"complementary_slices","probabilities":{"alternative_answers":0.0,"complementary_slices":1.0,"uncertain":0.0}}});
    let mut fields = json!({"snapshot_json":f["snapshot_json"],"program_json":p.to_string(),"trace_json":"[]","transition_count":245,"started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string()});
    let engine = WasmEngine::new().unwrap();
    let result = invoke(&engine, "semantic_step", fields.clone()).await;
    let after: Value =
        serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    eprintln!(
        "actual world set admission {}",
        after["world_set_admission"]
    );
    assert_eq!(result["callback_action"], "Reason");
    assert_eq!(result["callback_params"]["phase"], "compose");
    assert_eq!(after["world_set_admission"]["admitted"], true);
    fields["transition_count"] = json!(410);
    let stopped = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(stopped["callback_action"], "SearchPlanned");
    let after: Value =
        serde_json::from_str(stopped["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["world_set_admission"]["admitted"], false);
}

#[tokio::test]
async fn rejected_draft_names_causal_links_and_their_milestone_dates() {
    let engine = WasmEngine::new().unwrap();
    let mut fields = prepared(&engine).await;
    let mut snap: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    let mut worlds: Vec<Value> = snap["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "world")
        .cloned()
        .collect();
    for world in &mut worlds {
        world["id"] = world["Id"].clone();
    }
    worlds[0]["chain"][0]["id"] = json!("eu_rules_make_labels_visible");
    worlds[0]["chain"][0]["by"] = json!("2027-09-30");
    snap["world"]["target_date"] = json!("2027-09-30");
    worlds[0]["chain"][1]["id"] = json!("bad_outputs_create_receipts");
    worlds[0]["chain"][1]["by"] = json!("2027-05-31");
    snap["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["kind"] != "world");
    fields["snapshot_json"] = json!(snap.to_string());
    fields["reasoning_result"] =
        json!(json!({"baseline":answer(&snapshot())["baseline"],"worlds":worlds}).to_string());
    fields["phase"] = json!("compose");
    let result = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(result["callback_action"], "CompositionRejected");
    let p: Value =
        serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    let error = p["composition_correction"]["validation_error"]
        .as_str()
        .unwrap();
    for expected in [
        "eu_rules_make_labels_visible",
        "2027-09-30",
        "bad_outputs_create_receipts",
        "2027-05-31",
        "nondecreasing",
        "not event resolve_by",
    ] {
        assert!(error.contains(expected), "{error}");
    }
    assert!(result["callback_params"].get("snapshot_json").is_none());
    assert!(result["callback_params"].get("trace_json").is_none());
}

#[tokio::test]
async fn optional_invalid_replacement_falls_back_but_initial_composition_still_fails() {
    let engine = WasmEngine::new().unwrap();
    let host = Arc::new(WorldProvider::default());
    host.slices.store(true, Ordering::SeqCst);
    let mut fields = prepared(&engine).await;
    let recorded = call(&engine, fields.clone(), host.clone()).await;
    apply(&mut fields, &recorded);
    let request = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(request["callback_params"]["phase"], "compose");
    apply(&mut fields, &request);
    fields["phase"] = json!("compose");
    let original = fields.clone();
    fields["reasoning_result"] = json!("{malformed replacement");
    for _ in 0..2 {
        let rejected = invoke(&engine, "semantic_expand", fields.clone()).await;
        assert_eq!(rejected["callback_action"], "CompositionRejected");
        apply(&mut fields, &rejected);
    }
    let preserved = program(&fields);
    let fallback = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(fallback["callback_action"], "Expanded", "{fallback}");
    assert_eq!(
        fallback["callback_params"]["snapshot_json"],
        original["snapshot_json"]
    );
    assert_eq!(
        fallback["callback_params"]["started_at_ms"],
        original["started_at_ms"]
    );
    assert!(fallback["callback_params"].get("trace_json").is_none());
    apply(&mut fields, &fallback);
    let p = program(&fields);
    for key in [
        "results",
        "evaluations",
        "world_refinement",
        "world_set_audits",
        "active_world_ids",
        "world_revision",
        "tasks",
        "cursor",
    ] {
        assert_eq!(p[key], preserved[key], "{key}");
    }
    assert_eq!(
        p["world_set_audit"]["correction_status"],
        "correction_exhausted"
    );
    assert!(p["world_set_audit"]["correction_error"].as_str().is_some());
    let next = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(next["callback_action"], "Evaluate");
    let mut initial = fields.clone();
    let mut initial_program = program(&initial);
    initial_program["active_world_ids"] = json!([]);
    initial_program["world_set_audit"] = Value::Null;
    initial["program_json"] = json!(initial_program.to_string());
    let failed = invoke(&engine, "semantic_expand", initial).await;
    assert_eq!(failed["callback_action"], "Fail");
    let mut structurally_invalid = original.clone();
    structurally_invalid["reasoning_result"] = json!(json!({"worlds":[]}).to_string());
    for _ in 0..2 {
        let rejected = invoke(&engine, "semantic_expand", structurally_invalid.clone()).await;
        assert_eq!(rejected["callback_action"], "CompositionRejected");
        apply(&mut structurally_invalid, &rejected);
    }
    let fallback = invoke(&engine, "semantic_expand", structurally_invalid).await;
    assert_eq!(fallback["callback_action"], "Expanded");
    let p: Value = serde_json::from_str(
        fallback["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        p["world_set_audit"]["correction_status"],
        "correction_exhausted"
    );
    let mut oversized = original;
    oversized["reasoning_result"] =
        json!(json!({"worlds":[],"padding":"x".repeat(256*1024)}).to_string());
    let capped = invoke(&engine, "semantic_expand", oversized).await;
    assert_eq!(capped["callback_action"], "Expanded");
    let p: Value =
        serde_json::from_str(capped["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        p["world_set_audit"]["correction_status"],
        "correction_context_limit"
    );
}
