//! Branch generation crosses actual reasoning, expansion and Jev guest boundaries.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
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
        self.requests.lock().unwrap().push(request.clone());
        let answers: serde_json::Map<String, Value> = request["questions"]
            .as_object()
            .unwrap()
            .keys()
            .map(|key| (key.clone(), json!({"type":"noul","noul":0.23})))
            .collect();
        Ok((
            200,
            json!({"model":"jev-1.13.0","answers":answers}).to_string(),
        ))
    }
}
fn world(id: &str, components: Value) -> Value {
    let ids = components.as_array().unwrap();
    json!({"id":id,"trajectory_answer":"An entire trajectory under an explicit premise","title":"An entire trajectory","statement":"The defining changes occur together","mechanism":"These changes interact under their recorded premise","component_ids":components,"counter_ids":[],"scene":"An imagined day","narrative":"A different ordinary experience","what_you_can_do":[],"signals":["Watch the premise"],"falsifiers":["Premise fails"],"facets":ids.iter().enumerate().map(|(i,id)|json!({"id":format!("f{i}"),"title":format!("Consequence {i}"),"description":"A consequence","component_ids":[id]})).collect::<Vec<_>>(),"chain":[],"assumptions":[]})
}
#[tokio::test]
async fn layered_opposite_branches_survive_generation_evaluation_and_composition() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"Id":"w","description":"What changes?","last_ingest_date":"2026-09-30","target_date":"2027-09-30","hindcast_mode":"false"},"nodes":[{"Id":"e","kind":"evidence","statement":"Observed baseline","edges":"[]"},{"Id":"a","kind":"scenario","statement":"The shared premise occurs","edges":"[]"}]});
    let program = json!({"round":0,"calls":0,"http_calls":0,"cursor":0,"tasks":[],"results":{},"evaluations":{},"baseline":{"as_of":"2026-09-30","observed":[{"claim":"Observed baseline","evidence_ids":["e"]}],"assumptions":[],"unknowns":[]}});
    let hypotheses:Vec<_>=[("b","on"),("c","second"),("d","third"),("x","off"),("y","off"),("z","off")].iter().map(|(id,branch)|json!({"id":id,"title":format!("Consequence {id}"),"statement":format!("Consequence {id} occurs by the horizon"),"mechanism":"A concrete causal consequence","requires":[],"branch_id":branch})).collect();
    let draft = json!({"hypotheses":hypotheses,"research_evidence":[],"continue_exploring":true,"exploration_note":"Develop opposite premises and layered consequences","branches":[
 {"id":"on","condition":{"kind":"all_occurring","event_ids":["ref_0002"]},"by":"2027-01-01"},
 {"id":"second","parent_branch_id":"on","condition":{"kind":"all_occurring","event_ids":["b"]},"by":"2027-03-01"},
 {"id":"third","parent_branch_id":"second","condition":{"kind":"not_all_occurring","event_ids":["c","x"]},"by":"2027-06-01"},
 {"id":"off","condition":{"kind":"not_all_occurring","event_ids":["ref_0002"]},"by":"2027-01-01"}]});
    let fields = json!({"phase":"explore","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","started_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string(),"reasoning_result":draft.to_string()});

    for case in [
        "numeric_parent",
        "numeric_branch",
        "self",
        "foreign",
        "past",
        "cycle",
    ] {
        let mut invalid = draft.clone();
        match case {
            "numeric_parent" => invalid["branches"][0]["parent_branch_id"] = json!(42),
            "numeric_branch" => invalid["hypotheses"][0]["branch_id"] = json!(42),
            "self" => invalid["branches"][0]["condition"]["event_ids"] = json!(["b"]),
            "foreign" => invalid["branches"][0]["condition"]["event_ids"] = json!(["ref_0001"]),
            "past" => invalid["branches"][0]["by"] = json!("2020-01-01"),
            _ => invalid["branches"][0]["parent_branch_id"] = json!("second"),
        }
        let mut rejected_fields = fields.clone();
        rejected_fields["reasoning_result"] = json!(invalid.to_string());
        let rejected = run(
            &engine,
            "semantic_expand",
            rejected_fields,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(
            rejected["callback_action"], "CompositionRejected",
            "{case}: {rejected}"
        );
        assert!(rejected["callback_params"].get("snapshot_json").is_none());
        let retained: Value = serde_json::from_str(
            rejected["callback_params"]["program_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(retained["round"], program["round"]);
        assert_eq!(retained["results"], program["results"]);
    }
    let expanded = run(
        &engine,
        "semantic_expand",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(expanded["callback_action"], "Expanded", "{expanded}");
    let mut fields = expanded["callback_params"].clone();
    let snapshot: Value = serde_json::from_str(fields["snapshot_json"].as_str().unwrap()).unwrap();
    assert_eq!(snapshot["branches"].as_array().unwrap().len(), 4);
    let target = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["Id"] == "r1-d")
        .unwrap();
    assert_eq!(
        target["branch_state"]["history"],
        json!(["branch-r1-on", "branch-r1-second", "branch-r1-third"])
    );
    assert_eq!(
        target["branch_state"]["conditions"][2]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut program: Value =
        serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    for node in snapshot["nodes"].as_array().unwrap() {
        let id = node["Id"].as_str().unwrap();
        program["results"][id]["classify_temporal"] = json!("future_change");
        program["results"][id]["estimate_likelihood"] = json!("0.8");
    }
    let task = program["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["nodeId"] == "r1-d" && t["function"] == "estimate_conditional")
        .unwrap()
        .clone();
    program["tasks"] = json!([task]);
    program["cursor"] = json!(0);
    fields["program_json"] = json!(program.to_string());
    fields["trace_json"] = json!("[]");
    let host = Arc::new(Capture::default());
    let called = run(&engine, "semantic_call", fields.clone(), host.clone()).await;
    assert_eq!(called["callback_action"], "Recorded", "{called}");
    let next: Value =
        serde_json::from_str(called["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        next["results"]["r1-d"]["estimate_conditional"], "0.23",
        "{called}"
    );
    assert_eq!(next["results"]["r1-d"]["estimate_likelihood"], "0.8");
    let trace: Value =
        serde_json::from_str(called["callback_params"]["trace_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        trace[0]["request"]["state_ref"]["branch_state"],
        target["branch_state"]
    );
    assert!(
        host.requests.lock().unwrap()[0]
            .to_string()
            .contains("not_all_occurring")
    );
    assert_eq!(
        next["evaluations"]["r1-d"]["estimate_conditional"]["context"]["branch_state"],
        target["branch_state"]
    );
    fields["program_json"] = json!(next.to_string());
    // An expired resumed clock cannot run more provider work or erase completed judgments.
    let mut expired = fields.clone();
    expired["started_at_ms"] = json!("123");
    let mut pending = next.clone();
    pending["cursor"] = json!(0);
    expired["program_json"] = json!(pending.to_string());
    let stopped_host = Arc::new(Capture::default());
    let stopped = run(&engine, "semantic_call", expired, stopped_host.clone()).await;
    let stopped_program: Value =
        serde_json::from_str(stopped["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(stopped_program["stop_reason"], "time_budget");
    assert_eq!(stopped_program["results"], pending["results"]);
    assert_eq!(stopped_program["evaluations"], pending["evaluations"]);
    assert_eq!(stopped_program["tasks"], pending["tasks"]);
    assert!(stopped_host.requests.lock().unwrap().is_empty());
    // Producer receives exact retained branch state in its next ordinary generation input.
    fields["phase"] = json!("explore");
    let reasoning = run(
        &engine,
        "semantic_reasoning",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        reasoning["callback_action"], "LaunchReasoning",
        "{reasoning}"
    );
    assert!(reasoning.to_string().contains("branch-r1-third"));
    fields["phase"] = json!("compose");
    fields["reasoning_result"]=json!(json!({"shared_question":"How does the premise change the whole trajectory?","worlds":[world("on",json!(["r1-b","r1-c","r1-d"])),world("off",json!(["r1-x","r1-y","r1-z"]))]}).to_string());
    let composed = run(
        &engine,
        "semantic_expand",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(composed["callback_action"], "Expanded", "{composed}");
    let composed_snapshot: Value = serde_json::from_str(
        composed["callback_params"]["snapshot_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let worlds: Vec<_> = composed_snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "world")
        .collect();
    assert_eq!(worlds[0]["branch_conditions"].as_array().unwrap().len(), 3);
    assert_eq!(
        worlds[1]["branch_conditions"][0]["kind"],
        "not_all_occurring"
    );
    assert_eq!(
        worlds[1]["branch_conditions"][0]["events"][0]["statement"],
        "The shared premise occurs"
    );

    // Fresh whole-world odds include the conditions, not an assumed-true branch.
    let mut world_program: Value = serde_json::from_str(
        composed["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    world_program["tasks"] =
        json!([{"nodeId":"world-r1-off","function":"estimate_likelihood","depth":0}]);
    world_program["cursor"] = json!(0);
    let mut world_fields = composed["callback_params"].clone();
    world_fields["program_json"] = json!(world_program.to_string());
    world_fields["trace_json"] = json!("[]");
    let world_host = Arc::new(Capture::default());
    let world_call = run(
        &engine,
        "semantic_call",
        world_fields.clone(),
        world_host.clone(),
    )
    .await;
    assert_eq!(world_call["callback_action"], "Recorded", "{world_call}");
    let request = world_host.requests.lock().unwrap()[0].clone();
    assert!(request.to_string().contains("not conditions assumed true"));
    assert!(request.to_string().contains("branch_conditions"));
    let mut final_program: Value = serde_json::from_str(
        world_call["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    final_program["results"]["world-r1-on"]["estimate_likelihood"] = json!("0.41");
    let answer = json!({"schema":"foresight-worlds-v3","headline":"Opposite premises yield different trajectories","summary":"Layered consequences under different premises","horizon":"2027-09-30","probability_basis":"model_implied_world_estimate","probability_model":"overlapping_worlds","calibrated":false,"evidence_limits":["Deterministic integration fixture, not a live forecast"],"research_questions":[],"outcomes":worlds.iter().map(|w|json!({"id":w["Id"],"world_id":w["Id"],"title":w["title"],"definition":w["statement"],"component_ids":w["component_ids"],"counter_ids":[],"scene":w["scene"],"narrative":w["narrative"],"what_you_can_do":[],"signals":w["signals"],"falsifiers":w["falsifiers"]})).collect::<Vec<_>>()});
    world_fields["phase"] = json!("synthesize");
    world_fields["program_json"] = json!(final_program.to_string());
    world_fields["reasoning_result"] = json!(answer.to_string());
    let complete = run(
        &engine,
        "semantic_expand",
        world_fields,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(complete["callback_action"], "Complete", "{complete}");
    let answer: Value =
        serde_json::from_str(complete["callback_params"]["answer"].as_str().unwrap()).unwrap();
    assert_eq!(
        answer["outcomes"][1]["branch_conditions"],
        worlds[1]["branch_conditions"]
    );
    assert_eq!(answer["outcomes"][1]["probability"], 0.23);
    if let Ok(path) = std::env::var("ARN518_BRANCH_FIXTURE_OUTPUT") {
        std::fs::write(path,json!({"synthetic":true,"snapshot":composed_snapshot,"program":final_program,"answer":answer}).to_string()).unwrap();
    }
    // Contradictory branches cannot be composed together.
    fields["reasoning_result"]=json!(json!({"shared_question":"Same question","worlds":[world("bad",json!(["r1-b","r1-x","r1-y"])),world("off",json!(["r1-x","r1-y","r1-z"]))]}).to_string());
    let rejected = run(
        &engine,
        "semantic_expand",
        fields,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(rejected["callback_action"], "CompositionRejected");
    assert!(rejected["callback_params"].get("snapshot_json").is_none());
}
