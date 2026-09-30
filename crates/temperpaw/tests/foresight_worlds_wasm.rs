//! Independent whole-world contract tests against actual WASM; provider responses are fixtures.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn bytes(module: &str) -> Vec<u8> {
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
        let answers: serde_json::Map<String, Value> = request["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, q)| {
                let answer = if q["type"] == "noul" {
                    json!({"type":"noul","noul":0.23})
                } else {
                    let options = q["criteria"].as_object().unwrap();
                    let choice = if options.contains_key("compatible") {
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
#[tokio::test]
async fn fresh_world_reply_is_not_a_component_probability() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = snapshot();
    let host = Arc::new(WorldProvider::default());
    let program = json!({"baseline":answer(&snapshot)["baseline"],"cursor":0,"tasks":[{"nodeId":"w1","function":"estimate_likelihood","depth":1}],"results":{"h1":{"estimate_likelihood":"0.9"},"h2":{"estimate_likelihood":"0.8"}},"evaluations":{}});
    let hash = engine.compile_and_cache(&bytes("semantic_call")).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "world-test".into(),
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
    let recorded = invoke_with_host(&engine, "semantic_call", ctx, host.clone(), hash).await;
    assert_eq!(recorded["callback_action"], "Recorded", "{recorded}");
    let program: Value = serde_json::from_str(
        recorded["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(program["results"]["w1"]["estimate_likelihood"], "0.23");
    {
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let mut state = requests[0]["state"]["cases"]["q0"].clone();
        for (k, v) in requests[0]["state"]["common"].as_object().unwrap() {
            state[k] = v.clone();
        }
        assert_eq!(
            state["node"]["statement"],
            snapshot["nodes"][4]["statement"]
        );
        let encoded = state.to_string();
        for exact in [
            "The assistant sends appointment reminders today.",
            "2026-09-19",
            "Patients demand human approval by September 2027.",
        ] {
            assert!(
                encoded.contains(exact),
                "Missing actual evidence/counter in provider payload: {exact}"
            );
        }
    }
    let fields = json!({"phase":"synthesize","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"reasoning_result":answer(&snapshot).to_string()});
    let completed = invoke(&engine, "semantic_expand", fields).await;
    assert_eq!(completed["callback_action"], "Complete", "{completed}");
    let final_answer: Value =
        serde_json::from_str(completed["callback_params"]["answer"].as_str().unwrap()).unwrap();
    assert_eq!(final_answer["outcomes"][0]["probability"], 0.23);
    assert!(
        final_answer["outcomes"][1]["probability"].is_null(),
        "No score for w2; component estimates must not leak into world odds"
    );
    assert_eq!(final_answer["evaluation_status"], "partial");
    assert_eq!(final_answer["baseline"], answer(&snapshot)["baseline"]);
}
#[tokio::test]
async fn missing_world_scores_stay_null_and_baseline_cannot_cite_a_future() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = snapshot();
    let mut snapshot = snapshot;
    let mut archived = snapshot["nodes"][4].clone();
    archived["Id"] = json!("old-world");
    archived["archived"] = json!(true);
    snapshot["nodes"].as_array_mut().unwrap().push(archived);
    let mut draft = answer(&snapshot);
    let mut fields = json!({"phase":"synthesize","snapshot_json":snapshot.to_string(),"program_json":json!({"baseline":answer(&snapshot)["baseline"],"results":{"h1":{"estimate_likelihood":"0.9"},"h2":{"estimate_likelihood":"0.8"}}}).to_string(),"reasoning_result":draft.to_string()});
    let completed = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(completed["callback_action"], "Complete", "{completed}");
    let final_answer: Value =
        serde_json::from_str(completed["callback_params"]["answer"].as_str().unwrap()).unwrap();
    assert_eq!(final_answer["evaluation_status"], "unavailable");
    assert!(
        final_answer["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["probability"].is_null())
    );
    draft["baseline"]["observed"][0]["evidence_ids"] = json!(["h1"]);
    fields["reasoning_result"] = json!(draft.to_string());
    let mut invalid_program: Value =
        serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    invalid_program["baseline"] = draft["baseline"].clone();
    fields["program_json"] = json!(invalid_program.to_string());
    assert_eq!(
        invoke(&engine, "semantic_expand", fields).await["callback_action"],
        "Fail"
    );
}

#[tokio::test]
async fn composition_schedules_new_world_calls_not_component_reuse() {
    let engine = WasmEngine::new().unwrap();
    let full = snapshot();
    let mut snapshot = full.clone();
    snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["kind"] != "world");
    let worlds: Vec<Value> = [4usize, 5]
        .iter()
        .map(|&i| {
            let mut w = full["nodes"][i].clone();
            w["id"] = w["Id"].clone();
            w["title"] = json!("A new clinic morning");
            w["mechanism"] =
                json!("Changing both bookings and patient consent changes the staffing model.");
            w["scene"] = json!("The clinic opens with no booking calls waiting.");
            w["narrative"] =
                json!("Staff spend their morning helping patients instead of arranging times.");
            w["signals"] = json!(["Reception call volume falls"]);
            w["falsifiers"] = json!(["Manual booking remains necessary"]);
            w["what_you_can_do"] = json!(["Observe how the clinic handles mistakes"]);
            w
        })
        .collect();
    let old = json!({"cursor":0,"tasks":[],"results":{"h1":{"estimate_likelihood":"0.9"},"h2":{"estimate_likelihood":"0.8"}},"evaluations":{}});
    let mut generated = json!({"baseline":answer(&full)["baseline"],"worlds":worlds});
    let mut fields = json!({"phase":"compose","snapshot_json":snapshot.to_string(),"program_json":old.to_string(),"reasoning_result":generated.to_string()});
    let expanded = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(expanded["callback_action"], "Expanded", "{expanded}");
    let p: Value = serde_json::from_str(
        expanded["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(p["stage"], "worlds");
    let tasks = p["tasks"].as_array().unwrap();
    for id in ["world-r1-w1", "world-r1-w2"] {
        assert!(
            tasks
                .iter()
                .any(|t| t["nodeId"] == id && t["function"] == "estimate_likelihood")
        );
        assert!(
            tasks
                .iter()
                .any(|t| t["nodeId"] == id && t["function"] == "check_world_consistency")
        );
        assert!(p["results"][id]["estimate_likelihood"].is_null());
    }
    assert!(
        tasks
            .iter()
            .all(|t| t["nodeId"].as_str().unwrap().starts_with("world-")),
        "Components are already evaluated"
    );
    for links in [0, 1] {
        let mut parallel = generated.clone();
        for world in parallel["worlds"].as_array_mut().unwrap() {
            world["chain"].as_array_mut().unwrap().truncate(links);
        }
        let mut parallel_fields = fields.clone();
        parallel_fields["reasoning_result"] = json!(parallel.to_string());
        let accepted = invoke(&engine, "semantic_expand", parallel_fields).await;
        assert_eq!(
            accepted["callback_action"], "Expanded",
            "parallel components must not require invented links: {accepted}"
        );
        let program: Value = serde_json::from_str(
            accepted["callback_params"]["program_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let tasks = program["tasks"].as_array().unwrap();
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "check_pair")
                .count(),
            6
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "check_world_consistency")
                .count(),
            2
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "estimate_likelihood")
                .count(),
            2
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t["function"] == "conditional_on")
                .count(),
            links * 2
        );
    }
    for defect in ["cycle", "reversed_time", "bad_date", "uncovered_facet"] {
        let mut broken = generated.clone();
        match defect {
            "cycle" => broken["worlds"][0]["chain"][0]["from_ids"] = json!(["h3"]),
            "reversed_time" => broken["worlds"][0]["chain"][1]["by"] = json!("2027-01-01"),
            "bad_date" => broken["worlds"][0]["chain"][0]["by"] = json!("2027-02-30"),
            _ => broken["worlds"][0]["facets"][2]["component_ids"] = json!(["h1"]),
        }
        fields["reasoning_result"] = json!(broken.to_string());
        assert_eq!(
            invoke(&engine, "semantic_expand", fields.clone()).await["callback_action"],
            "CompositionRejected",
            "{defect}"
        );
    }
    generated["baseline"]["observed"][0]["evidence_ids"] = json!(["h1"]);
    fields["reasoning_result"] = json!(generated.to_string());
    for expected in ["CompositionRejected", "CompositionRejected", "Fail"] {
        let rejected = invoke(&engine, "semantic_expand", fields.clone()).await;
        assert_eq!(rejected["callback_action"], expected);
        if expected != "Fail" {
            fields["program_json"] = rejected["callback_params"]["program_json"].clone();
        }
    }
}

#[tokio::test]
async fn structural_fanout_preserves_cases_and_separates_dependent_likelihood() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = snapshot();
    let host = Arc::new(WorldProvider::default());
    let tasks = json!([
      {"nodeId":"pair:2:h1:2:h2","function":"check_pair","pair_ids":["h1","h2"],"depth":0},
      {"nodeId":"pair:2:h1:2:h3","function":"check_pair","pair_ids":["h1","h3"],"depth":0},
      {"nodeId":"w1/link/a","world_id":"w1","link_id":"a","function":"conditional_on","depth":0},
      {"nodeId":"w1","world_id":"w1","function":"check_world_consistency","depth":0},
      {"nodeId":"w1","function":"estimate_likelihood"}
    ]);
    let p = json!({"cursor":0,"tasks":tasks,"baseline":answer(&snapshot)["baseline"],"results":{},"evaluations":{}});
    let hash = engine.compile_and_cache(&bytes("semantic_call")).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "test".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "batch-test".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("semantic_call".into()),
        trigger_params: json!({}),
        entity_state: json!({"fields":{"snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"trace_json":"[]"}}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("typesafe_api_key".into(), "fixture-only".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let mut ctx = ctx;
    let mut r = json!({});
    for _ in 0..5 {
        r = invoke_with_host(
            &engine,
            "semantic_call",
            ctx.clone(),
            host.clone(),
            hash.clone(),
        )
        .await;
        assert_eq!(r["callback_action"], "Recorded", "{r}");
        let p: Value =
            serde_json::from_str(r["callback_params"]["program_json"].as_str().unwrap()).unwrap();
        if p["cursor"] == 5 {
            break;
        }
        assert!(p["cursor"].as_u64().unwrap() < 5);
        ctx.entity_state["fields"]["program_json"] = r["callback_params"]["program_json"].clone();
        ctx.entity_state["fields"]["trace_json"] = r["callback_params"]["trace_json"].clone();
    }
    let p: Value =
        serde_json::from_str(r["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    let t: Value =
        serde_json::from_str(r["callback_params"]["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["cursor"], 5);
    assert_eq!(t.as_array().unwrap().len(), 5);
    let requests = host.requests.lock().unwrap();
    assert_eq!(p["http_calls"].as_u64().unwrap() as usize, requests.len());
    assert_eq!(requests[0]["questions"].as_object().unwrap().len(), 2);
    assert_eq!(requests[0]["state"]["cases"]["q0"]["events"][1]["Id"], "h2");
    assert_eq!(requests[0]["state"]["cases"]["q1"]["events"][1]["Id"], "h3");
    let conditional = requests
        .iter()
        .flat_map(|r| r["state"]["cases"].as_object().unwrap().values())
        .find(|c| c["target_event"]["Id"] == "h2")
        .unwrap();
    assert_eq!(conditional["prerequisite_events"][0]["Id"], "h1");
    assert!(conditional["world"]["component_ids"].is_null());
    let last = requests.last().unwrap();
    assert_eq!(last["questions"].as_object().unwrap().len(), 1);
    assert_eq!(
        last["state"]["cases"]["q0"]["assessment"]["check_world_consistency"],
        "compatible"
    );
    for i in 0..4 {
        assert!(t[i]["response"]["answers"]["result"].is_object());
        assert!(t[i]["httpCallId"].as_u64().unwrap() < t[4]["httpCallId"].as_u64().unwrap());
    }
    assert_eq!(t[4]["questionKey"], "q0");
    assert_ne!(t[0]["caseHash"], t[1]["caseHash"]);
    assert_eq!(t[0]["requestHash"], t[1]["requestHash"]);
}

#[tokio::test]
async fn combinations_precede_composition_and_conflicts_force_bounded_revision() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = snapshot();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let p = json!({"stage":"exploration","cursor":0,"tasks":[],"continue_exploring":false,"independent_challenge":{"status":"completed"},"results":{},"evaluations":{}});
    let mut fields = json!({"snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"trace_json":"[]","started_at_ms":now.to_string()});
    let planned = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(planned["callback_action"], "SearchPlanned", "{planned}");
    let p: Value =
        serde_json::from_str(planned["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["stage"], "combinations");
    assert_eq!(p["tasks"].as_array().unwrap().len(), 6);
    fields["program_json"] = planned["callback_params"]["program_json"].clone();
    assert_eq!(
        invoke(&engine, "semantic_step", fields.clone()).await["callback_action"],
        "Evaluate"
    );
    let mut p = json!({"stage":"worlds","cursor":0,"tasks":[],"world_revision":1,"active_world_ids":["w1"],"results":{"w1":{"check_world_consistency":"conflict"}},"evaluations":{}});
    fields["program_json"] = json!(p.to_string());
    let revise = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(revise["callback_action"], "Reason");
    assert_eq!(revise["callback_params"]["phase"], "compose");
    p["world_revision"] = json!(3);
    fields["program_json"] = json!(p.to_string());
    let stop = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(stop["callback_params"]["phase"], "synthesize");
    let p: Value =
        serde_json::from_str(stop["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(p["world_audits"]["w1"]["status"], "conflicts_found");
}

#[tokio::test]
async fn new_source_rechecks_the_same_hypothesis_instead_of_inheriting_its_score() {
    let engine = WasmEngine::new().unwrap();
    let mut snapshot = snapshot();
    snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["kind"] != "world");
    let p = json!({"round":1,"evidence_ids":["e"],"cursor":0,"tasks":[],"results":{"h1":{"estimate_likelihood":"0.9","classify_gap":"none"}},"evaluations":{"h1":{"estimate_likelihood":{"probability":0.9}}}});
    let result = json!({"hypotheses":[],"research_evidence":[{"id":"new-source","statement":"A second clinic reports patients refusing automated changes.","url":"https://example.org/new-observation","quote":"Patients asked to approve every change.","observed_at":"2026-09-20","provenance":"observed"}],"continue_exploring":false,"exploration_note":"Contrary evidence needs evaluation."});
    let fields = json!({"phase":"explore","snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"reasoning_result":result.to_string()});
    let r = invoke(&engine, "semantic_expand", fields).await;
    assert_eq!(r["callback_action"], "Expanded", "{r}");
    let p: Value =
        serde_json::from_str(r["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert!(p["results"]["h1"]["estimate_likelihood"].is_null());
    assert!(
        p["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["nodeId"] == "h1" && t["function"] == "estimate_likelihood")
    );
}
