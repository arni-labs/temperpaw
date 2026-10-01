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

fn parsed(v: &Value, key: &str) -> Value {
    serde_json::from_str(v[key].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn scope_repair_precedes_checks_and_preserves_clock_history_and_failed_drafts() {
    let engine = WasmEngine::new().unwrap();
    let metadata = json!({"kind":"finding","publication_date":"2020","observation_period":{"start":"2019","end":"2020"},"retrieved_at":"2026-10-01"});
    let snapshot = json!({"world":{"Id":"w","description":"How might teenagers learn?","last_ingest_date":"2026-10-01","target_date":"2031-10-01","hindcast_mode":"false","evidence_contract":"v1"},"nodes":[{"Id":"e","kind":"evidence","statement":"A historical scoped observation","edges":"[]","evidence_metadata":metadata}]});
    let baseline = json!({"as_of":"2026-10-01","observed":[{"claim":"Historical limited observation","evidence_ids":["e"]}],"assumptions":[],"unknowns":["Sources cover only one setting."]});
    let review = json!({"requested_question":"How might teenagers learn?","evidence_scope":"One setting","narrowing_basis":"evidence_availability","status":"narrowed","limitations":["Sources cover only one setting."]});
    let program = json!({"round":0,"rounds":[],"cursor":0,"tasks":[{"nodeId":"e","function":"classify_gap","depth":0}],"results":{},"evaluations":{},"evidence_ids":["e"],"http_calls":0,"baseline":baseline,"baseline_status":"established","scope_review":review,"scope_repair":{"status":"pending","attempted":false},"continue_exploring":true});
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let trace = json!([{"historical":"immutable fixture receipt"}]);
    let mut fields = json!({"phase":"seed","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":trace.to_string(),"started_at_ms":clock,"transition_count":10});
    for frozen in [true, false] {
        let mut limited = fields.clone();
        if frozen {
            let mut view = snapshot.clone();
            view["world"]["hindcast_mode"] = json!("true");
            limited["snapshot_json"] = json!(view.to_string());
        } else {
            limited["transition_count"] = json!(230);
        }
        let skipped = run(
            &engine,
            "semantic_step",
            limited.clone(),
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert!(
            skipped["callback_params"]["program_json"].is_string(),
            "{skipped}"
        );
        let p = parsed(&skipped["callback_params"], "program_json");
        assert_eq!(p["scope_repair"]["status"], "skipped", "{skipped}");
        assert_eq!(p["scope_repair"]["attempted"], false);
        assert_eq!(p["baseline"], baseline);
        assert_eq!(skipped["callback_action"], "SearchPlanned");
        let mut continued_fields = limited.clone();
        continued_fields["transition_count"] =
            json!(limited["transition_count"].as_u64().unwrap() + 1);
        continued_fields["program_json"] = skipped["callback_params"]["program_json"].clone();
        let continued = run(
            &engine,
            "semantic_step",
            continued_fields,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(
            continued["callback_action"], "Evaluate",
            "skip must persist once: {continued}"
        );
    }
    let stepped = run(
        &engine,
        "semantic_step",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        stepped["callback_action"], "Reason",
        "pending scope repair must run before the ordinary evidence check: {stepped}"
    );
    assert_eq!(stepped["callback_params"]["phase"], "explore");
    assert_eq!(parsed(&stepped["callback_params"], "trace_json"), trace);
    fields["phase"] = json!("explore");
    fields["program_json"] = stepped["callback_params"]["program_json"].clone();
    let reasoning = run(
        &engine,
        "semantic_reasoning",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(reasoning["callback_action"], "LaunchReasoning");
    assert!(
        !reasoning["callback_params"]["system_prompt"]
            .as_str()
            .unwrap()
            .contains("Develop layered consequences")
    );
    let prompt = reasoning["callback_params"]["system_prompt"]
        .as_str()
        .unwrap();
    let contract_text = prompt
        .split(
            "Scope output contract (scope_review and scope_disposition are distinct judgments): ",
        )
        .nth(1)
        .expect("repair producer must supply canonical full output contract")
        .split("\n\nTreat response_correction")
        .next()
        .unwrap();
    let contract: Value = serde_json::from_str(contract_text).unwrap();
    assert_eq!(
        contract["scope"]["scope_review"]["status"]["enum"],
        json!(["aligned", "narrowed", "uncertain"])
    );
    assert_eq!(
        contract["scope"]["scope_review"]["narrowing_basis"]["enum"],
        json!(["user_explicit", "evidence_availability", "none"])
    );
    assert_eq!(
        contract["scope"]["scope_disposition"]["status"]["enum"],
        json!(["addressed", "limited", "uncertain"])
    );
    assert_eq!(contract["baseline"]["observed"]["maxItems"], 16);
    assert_eq!(
        contract["baseline"]["observed"]["items"]["claim"]["maxLength"],
        400
    );
    let mut refreshed = baseline.clone();
    refreshed["observed"] =
        json!([{"claim":"Corrected historical observation","evidence_ids":["source"]}]);
    let draft = json!({"hypotheses":[],"branches":[],"research_evidence":[{"id":"source","statement":"A source-backed historical observation","url":"https://example.org/study","quote":"Historical source text","provenance":"observed","evidence_metadata":metadata}],"continue_exploring":true,"exploration_note":"Scope remains limited","baseline":refreshed,"scope_review":review,"scope_disposition":{"status":"limited","report":"Added a source and corrected a claim; limitations remain.","evidence_ids":["ref_0001","source"]}});
    fields["reasoning_result"] = json!(draft.to_string());
    for invalid in ["future", "namespace_collision"] {
        let mut bad = fields.clone();
        let mut value = draft.clone();
        if invalid == "future" {
            value["research_evidence"][0]["evidence_metadata"]["publication_date"] = json!("2027");
        } else {
            value["research_evidence"][0]["id"] = json!("e");
        }
        bad["reasoning_result"] = json!(value.to_string());
        let rejected = run(
            &engine,
            "semantic_expand",
            bad,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(
            rejected["callback_action"], "CompositionRejected",
            "{invalid}: {rejected}"
        );
        assert!(rejected["callback_params"].get("snapshot_json").is_none());
        assert_eq!(
            parsed(&rejected["callback_params"], "program_json")["baseline"],
            baseline
        );
    }
    for (field, bad_value, expected) in [
        (
            "status",
            "limited",
            "Invalid scope_review.status; accepted values: aligned, narrowed, uncertain",
        ),
        (
            "narrowing_basis",
            "original_question_retained_with_limited_source_repair",
            "Invalid scope_review.narrowing_basis; accepted values: user_explicit, evidence_availability, none",
        ),
    ] {
        let mut bad = fields.clone();
        let mut v = draft.clone();
        v["scope_review"][field] = json!(bad_value);
        bad["reasoning_result"] = json!(v.to_string());
        let rejected = run(
            &engine,
            "semantic_expand",
            bad,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(rejected["callback_action"], "CompositionRejected");
        let correction = parsed(&rejected["callback_params"], "program_json");
        assert_eq!(
            correction["response_correction"]["validation_error"],
            expected
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                correction["response_correction"]["rejected_draft"]
                    .as_str()
                    .unwrap()
            )
            .unwrap(),
            v
        );
        let mut retry = fields.clone();
        retry["program_json"] = rejected["callback_params"]["program_json"].clone();
        let producer = run(
            &engine,
            "semantic_reasoning",
            retry,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        let input: Value = serde_json::from_str(
            producer["callback_params"]["user_message"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let data = &input["response_correction"];
        assert!(
            !producer["callback_params"]["system_prompt"]
                .as_str()
                .unwrap()
                .contains("Historical source text")
        );
        assert_eq!(
            data["rejected_draft"],
            correction["response_correction"]["rejected_draft"]
        );
    }
    let mut too_many = fields.clone();
    let mut draft17 = draft.clone();
    draft17["baseline"]["observed"] = json!(vec![draft["baseline"]["observed"][0].clone(); 17]);
    too_many["reasoning_result"] = json!(draft17.to_string());
    let rejected = run(
        &engine,
        "semantic_expand",
        too_many,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(rejected["callback_action"], "CompositionRejected");
    assert_eq!(
        parsed(&rejected["callback_params"], "program_json")["response_correction"]["validation_error"],
        "baseline.observed must contain 0–16 observations"
    );
    let mut malformed = fields.clone();
    malformed["reasoning_result"] = json!("{invalid source data");
    let rejected = run(
        &engine,
        "semantic_expand",
        malformed,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(
        parsed(&rejected["callback_params"], "program_json")["response_correction"]["rejected_draft"],
        "{invalid source data"
    );
    let mut oversized = fields.clone();
    let mut large = draft.clone();
    large["scope_review"]["status"] = json!("limited");
    large["unused_source_payload"] = json!("x".repeat(256 * 1024));
    oversized["reasoning_result"] = json!(large.to_string());
    let refused = run(
        &engine,
        "semantic_expand",
        oversized,
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(refused["callback_action"], "Expanded");
    assert_eq!(
        parsed(&refused["callback_params"], "snapshot_json"),
        snapshot
    );
    let receipt = parsed(&refused["callback_params"], "program_json");
    assert!(
        receipt["scope_repair"]["report"]
            .as_str()
            .unwrap()
            .contains("exceeds 256 KiB correction context limit")
    );
    assert!(
        !receipt["scope_repair"]["report"]
            .as_str()
            .unwrap()
            .contains("after bounded corrections")
    );
    let expanded = run(
        &engine,
        "semantic_expand",
        fields.clone(),
        Arc::new(SimWasmHost::new()),
    )
    .await;
    assert_eq!(expanded["callback_action"], "Expanded", "{expanded}");
    let result = &expanded["callback_params"];
    let after = parsed(result, "snapshot_json");
    let done = parsed(result, "program_json");
    assert_eq!(result["started_at_ms"], clock);
    assert_eq!(after["nodes"][0], snapshot["nodes"][0]);
    assert_eq!(after["nodes"][1]["Id"], "scope-source");
    assert_eq!(after["nodes"][1]["evidence_metadata"], metadata);
    assert_eq!(done["round"], 0);
    assert_eq!(done["scope_repair"]["original_baseline"], baseline);
    assert_eq!(done["scope_repair"]["status"], "completed");
    assert_eq!(
        done["scope_repair"]["evidence_ids"],
        json!(["e", "scope-source"])
    );
    let mut next = fields.clone();
    next["snapshot_json"] = result["snapshot_json"].clone();
    next["program_json"] = result["program_json"].clone();
    let continued = run(&engine, "semantic_step", next, Arc::new(SimWasmHost::new())).await;
    assert_eq!(
        continued["callback_action"], "Evaluate",
        "repair must not repeat: {continued}"
    );
    // Rejected typed input and invalid JSON both exhaust the existing correction bound atomically.
    for invalid in ["not JSON".to_owned(), {
        let mut v = draft.clone();
        v["hypotheses"] = json!([{"id":"forbidden"}]);
        v.to_string()
    }] {
        let mut bad = fields.clone();
        let mut p = parsed(&bad, "program_json");
        p["response_correction"] = json!({"attempt":2});
        bad["program_json"] = json!(p.to_string());
        bad["reasoning_result"] = json!(invalid);
        let fallback = run(
            &engine,
            "semantic_expand",
            bad,
            Arc::new(SimWasmHost::new()),
        )
        .await;
        assert_eq!(fallback["callback_action"], "Expanded", "{fallback}");
        assert_eq!(
            parsed(&fallback["callback_params"], "snapshot_json"),
            snapshot
        );
        let limited = parsed(&fallback["callback_params"], "program_json");
        assert_eq!(limited["scope_repair"]["status"], "failed");
        assert_eq!(limited["baseline"], baseline);
        assert_eq!(limited["round"], 0);
        assert_eq!(fallback["callback_params"]["started_at_ms"], clock);
    }
}
