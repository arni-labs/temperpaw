use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};
async fn invoke(engine: &WasmEngine, module: &str, fields: &Value, response: &Value) -> Value {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path=std::env::var_os("FORESIGHT_WASM_DIR").map(std::path::PathBuf::from).map(|p|p.join(format!("{module}.wasm"))).unwrap_or_else(||root.join(format!("os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm")));
    let bytes = std::fs::read(path).unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "fixture".into(),
        entity_type: "SemanticRun".into(),
        entity_id: "proposal-quality".into(),
        trigger_action: "Next".into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"counters":{"transition_count":fields["transition_count"]},"fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("temper_api_url".into(), "http://fixture".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let result = engine
        .invoke(
            &hash,
            &ctx,
            Arc::new(SimWasmHost::new().with_default_response(200, &response.to_string())),
            &WasmResourceLimits {
                max_memory: 256 * 1024 * 1024,
                max_fuel: 10_000_000_000,
                ..Default::default()
            },
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    serde_json::to_value(result).unwrap()
}

#[tokio::test]
#[ignore = "Requires the authorized captured games proposal checkpoint"]
async fn captured_unchecked_game_pool_keeps_reserved_deadline_reason() {
    let path = std::env::var("FORESIGHT_PROPOSAL_CAPTURE").expect("captured program JSON path");
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut before = saved.clone();
    // Reconstruct the scheduler input immediately before its captured finish callback.
    // Actual candidate/source/task contents remain unchanged; only finish outputs are undone.
    before["tasks"] = before["endpoint_proposal_attempt"]["tasks"].clone();
    before["endpoint_proposal_attempt"]["status"] = json!("checking");
    before["endpoint_proposal_attempt"]["checks"] = json!([]);
    before["endpoint_proposal_history"] = json!([]);
    before["stop_reason"] = json!("");
    assert_eq!(before["tasks"].as_array().unwrap().len(), 33);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let snapshot = json!({"world":before["endpoint_proposal_attempt"]["world"],"nodes":[]});
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":before.to_string(),"trace_json":"[]","transition_count":67,"started_at_ms":now.saturating_sub(3_000_000).to_string()});
    let engine = WasmEngine::new().unwrap();
    let first = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(first["callback_action"], "SearchPlanned", "{first}");
    let after: Value =
        serde_json::from_str(first["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["stop_reason"], "time_budget");
    assert_eq!(after["endpoint_proposal_attempt"]["status"], "unresolved");
    assert_eq!(
        after["endpoint_proposal_attempt"]["checks"]
            .as_array()
            .unwrap()
            .len(),
        33
    );
    assert!(
        after["endpoint_proposal_attempt"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["result"].is_null() && c["evaluation"].is_null())
    );
    assert_eq!(after["results"], before["results"]);
    assert_eq!(after["evaluations"], before["evaluations"]);
    assert_eq!(after["proposal_pool"], before["proposal_pool"]);
    assert!(first["callback_params"].get("started_at_ms").is_none());
    let mut next = fields.clone();
    next["program_json"] = json!(after.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert_eq!(terminal["callback_action"], "Fail");
    let msg = terminal["callback_params"]["error_message"]
        .as_str()
        .unwrap();
    assert!(
        msg.contains("time_budget") && msg.contains("0 of 33"),
        "{msg}"
    );
    assert!(!msg.contains("did not produce sufficiently distinct"));
    for reason in [
        "call_budget",
        "transition_budget",
        "trace_budget",
        "provider_error",
    ] {
        let mut resource = after.clone();
        resource["stop_reason"] = json!(reason);
        next["program_json"] = json!(resource.to_string());
        let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
        assert_eq!(terminal["callback_action"], "Fail");
        assert!(
            terminal["callback_params"]["error_message"]
                .as_str()
                .unwrap()
                .contains(reason)
        );
    }
    let mut contrast = before.clone();
    contrast["proposal_pool"]["stage"] = json!("contrast");
    next["program_json"] = json!(contrast.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert_eq!(terminal["callback_action"], "Fail");
    assert!(
        terminal["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("time_budget")
    );
    let mut quality = after.clone();
    quality["stop_reason"] = json!("endpoint_proposal_quality");
    next["program_json"] = json!(quality.to_string());
    let terminal = invoke(&engine, "semantic_step", &next, &json!({})).await;
    assert!(
        terminal["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("Endpoint proposal quality unresolved")
    );
}

#[tokio::test]
async fn contrast_resource_failure_never_reports_absent_or_stale_check_counts() {
    let engine = WasmEngine::new().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    for prior_attempt in [
        Value::Null,
        json!({"tasks":vec![json!({});33],"checks":vec![json!({"evaluation":{"type":"choice"}});33],"status":"examined"}),
    ] {
        let program = json!({"stage":"proposals","endpoint_proposal_contract":2,"proposal_pool":{"stage":"contrast"},"tasks":[],"cursor":0,"results":{},"endpoint_proposal_attempt":prior_attempt});
        let fields = json!({"snapshot_json":json!({"world":{},"nodes":[]}).to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":67,"started_at_ms":now.saturating_sub(3_000_000).to_string()});
        let out = invoke(&engine, "semantic_step", &fields, &json!({})).await;
        assert_eq!(out["callback_action"], "Fail", "{out}");
        let message = out["callback_params"]["error_message"].as_str().unwrap();
        assert!(
            message.contains("research remains incomplete") && message.contains("time_budget"),
            "{message}"
        );
        assert!(message.contains("No current comparison checks were scheduled"));
        assert!(!message.contains("0 of 0") && !message.contains("33 of 33"));
    }
}

#[tokio::test]
#[ignore = "Requires local deterministic deferred producer fixture"]
async fn deferred_native_completion_preserves_paths_and_original_clock() {
    let fixture: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_DEFERRED_FIXTURE").unwrap()).unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let mut program = fixture["passed"].clone();
    // Replay the actual step boundary with the recorded deterministic judgments.
    program["stage"] = json!("proposals");
    program["endpoint_proposal_attempt"]["status"] = json!("checking");
    program["tasks"] = program["endpoint_proposal_attempt"]["tasks"].clone();
    program["cursor"] = json!(program["tasks"].as_array().unwrap().len());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let original_start = now.saturating_sub(2_400_000).to_string();
    let fields = json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":320,"started_at_ms":original_start});
    let mut pending_fields = fields.clone();
    pending_fields["program_json"] = json!(fixture["checking"].to_string());
    let scheduled = invoke(&engine, "semantic_step", &pending_fields, &json!({})).await;
    assert_eq!(scheduled["callback_action"], "Evaluate", "{scheduled}");
    assert!(scheduled["callback_params"].get("started_at_ms").is_none());
    let result = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(result["callback_action"], "SearchPlanned", "{result}");
    let completed: Value = serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(completed["endpoint_search"], program["endpoint_search"]);
    assert_eq!(completed["endpoint_novelty"], program["endpoint_novelty"]);
    assert_eq!(completed["deferred_novelty_recheck"]["status"], "completed");
    assert_eq!(completed["stage"], "exploration");
    assert_eq!(completed["transition_count"], 320);
    assert!(result["callback_params"].get("started_at_ms").is_none());
}

#[tokio::test]
#[ignore = "Requires authorized captured pass9 proposal checkpoint"]
async fn captured_checked_selection_enters_backward_without_rewriting_worlds() {
    let response: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_PASS9_CAPTURE").unwrap()).unwrap()).unwrap();
    let rows: Value = serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let captured = &rows[0];
    let mut program: Value = serde_json::from_str(captured["program_json"].as_str().unwrap()).unwrap();
    let pair = program["endpoint_proposal_history"].as_array().unwrap().iter().find(|a|a["pool_stage"] == "pairs").unwrap().clone();
    let snapshot = json!({"world":pair["world"],"nodes":pair["source_evidence"]});
    // Reconstruct the recorded checkpoint before pair completion. Do not call
    // the provider or change the live run; recorded Jev checks remain unchanged.
    program["endpoint_proposal_attempt"] = pair.clone();
    program["endpoint_proposal_attempt"]["status"] = json!("checking");
    program["endpoint_proposal_history"] = json!([program["endpoint_proposal_history"][0]]);
    program["tasks"] = pair["tasks"].clone();
    program["cursor"] = json!(pair["tasks"].as_array().unwrap().len());
    program["stage"] = json!("proposals");
    program["proposal_pool"]["stage"] = json!("pairs");
    program["proposal_pool"]["candidates"] = pair["endpoints"].clone();
    program["proposal_pool"].as_object_mut().unwrap().remove("development");
    program.as_object_mut().unwrap().remove("endpoint_search");
    for check in pair["checks"].as_array().unwrap() {
        let task = &check["task"];
        program["results"][task["nodeId"].as_str().unwrap()][task["function"].as_str().unwrap()] = check["result"].clone();
        program["evaluations"][task["nodeId"].as_str().unwrap()][task["function"].as_str().unwrap()] = check["evaluation"].clone();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    // Offline simulator clock, not a live run retry or deadline change.
    let mut fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":captured["counters"]["transition_count"],"started_at_ms":now.saturating_sub(1_800_000).to_string()});
    let engine = WasmEngine::new().unwrap();
    let selected = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(selected["callback_action"], "SearchPlanned", "{selected}");
    let frozen: Value = serde_json::from_str(selected["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(frozen["stage"], "exploration");
    assert!(frozen["proposal_pool"]["development"].is_null());
    assert_eq!(frozen["endpoint_novelty"], program["endpoint_novelty"]);
    for endpoint in frozen["endpoint_search"]["endpoints"].as_array().unwrap() {
        assert_eq!(Some(endpoint), pair["endpoints"].as_array().unwrap().iter().find(|e|e["id"]==endpoint["id"]));
    }
    fields["program_json"] = selected["callback_params"]["program_json"].clone();
    let next = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(next["callback_action"], "Reason", "{next}");
    assert_eq!(next["callback_params"]["phase"], "backward");
    assert!(next["callback_params"].get("started_at_ms").is_none());
}

#[tokio::test]
async fn combined_scope_and_optional_repair_use_route_preserving_dispatch() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"description":"Question","hindcast_mode":"false"},"nodes":[]});
    let mut program = json!({"stage":"exploration","endpoint_proposal_contract":2,"world_search_contract":1,"scope_repair":{"status":"pending"},"tasks":[],"cursor":0,"results":{},"evaluations":{},"round":0});
    let mut fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":4,"started_at_ms":now.to_string()});
    let first = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(first["callback_action"], "Reason", "{first}");
    assert_eq!(
        first["callback_params"]["phase"], "imagine",
        "Scope gaps must travel with contrast, not a separate research child"
    );
    program["scope_repair"]["status"] = json!("completed");
    program["stage"] = json!("proposals");
    program["proposal_pool"] = json!({"stage":"contrast","research_attempts":1,"novelty_repair":{"status":"pending"},"selected_ids":["a","b","c"],"candidates":[{"id":"a"},{"id":"b"},{"id":"c"}]});
    program["reasoning_durations_ms"] = json!({"explore":663216});
    fields["program_json"] = json!(program.to_string());
    fields["started_at_ms"] = json!(now.saturating_sub(1_205_400).to_string());
    fields["transition_count"] = json!(66);
    let second = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(second["callback_action"], "SearchPlanned", "{second}");
    let saved: Value =
        serde_json::from_str(second["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        saved["proposal_pool"]["repair_time_admission"]["admitted"],
        false
    );
    assert_eq!(
        saved["proposal_pool"]["novelty_repair"]["status"],
        "not_admitted"
    );
    assert_eq!(
        saved["endpoint_search"]["endpoints"],
        program["proposal_pool"]["candidates"]
    );
    assert!(second["callback_params"].get("started_at_ms").is_none());
}

#[tokio::test]
async fn city_required_comparison_and_refusal_receipt_use_native_callbacks() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"description":"City question"},"nodes":[]});
    let mut program = json!({"stage":"proposals","endpoint_proposal_contract":2,"world_search_contract":1,"tasks":[],"cursor":0,"round":0,"results":{},"evaluations":{},"reasoning_durations_ms":{"explore":541525,"imagine":240950},"proposal_pool":{"stage":"contrast","research_attempts":1,"development":{"status":"completed"},"development_admission":{"admitted":true},"candidates":[{"id":"preserved"}]}});
    let mut fields = json!({"snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","transition_count":63,"started_at_ms":now.saturating_sub(1_147_099).to_string()});
    let required = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(required["callback_action"], "Reason");
    assert_eq!(required["callback_params"]["phase"], "explore");
    program["proposal_pool"]["creative_repair"] = json!({"status":"pending"});
    fields["program_json"] = json!(program.to_string());
    let refused = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(refused["callback_action"], "SearchPlanned");
    let saved: Value =
        serde_json::from_str(refused["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        saved["proposal_pool"]["comparison_admission"]["reason"],
        "time_budget"
    );
    assert_eq!(
        saved["proposal_pool"]["candidates"],
        program["proposal_pool"]["candidates"]
    );
    fields["program_json"] = refused["callback_params"]["program_json"].clone();
    let terminal = invoke(&engine, "semantic_step", &fields, &json!({})).await;
    assert_eq!(terminal["callback_action"], "Fail");
    assert!(
        terminal["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("time_budget")
    );
    assert!(required["callback_params"].get("started_at_ms").is_none());
}

#[tokio::test]
async fn oversized_mandatory_work_never_applies_and_correction_cannot_refresh_capacity() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"description":"What changes by2035?","last_ingest_date":"2026-10-03","target_date":"2035-12-31","hindcast_mode":"false"},"nodes":[]});
    let program = json!({"audit_policy_version":2,"world_search_contract":1,"stage":"exploration","round":0,"rounds":[],"tasks":[],"cursor":0,"results":{},"evaluations":{},"endpoint_search":{"endpoints":[],"routes":[],"amendments":[],"rounds":[]},"baseline":{"as_of":"2026-10-03","observed":[],"assumptions":[],"unknowns":["Present remains uncertain"]},"admitted_work":{"admitted":true,"status":"generating","evaluation_transition_capacity":0}});
    let draft = json!({"hypotheses":[{"id":"candidate","statement":"By2035 people can carry a shared virtual place between independent environments","requires":[]}],"research_evidence":[],"routes":[],"amendments":[],"continue_exploring":false,"exploration_note":"Capacity boundary fixture"});
    let mut fields = json!({"phase":"backward","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","reasoning_result":draft.to_string(),"transition_count":100,"started_at_ms":now.to_string()});
    for attempt in 1..=2 {
        let rejected = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
        assert_eq!(
            rejected["callback_action"], "CompositionRejected",
            "{rejected}"
        );
        assert!(rejected["callback_params"].get("snapshot_json").is_none());
        let saved: Value = serde_json::from_str(
            rejected["callback_params"]["program_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(
            saved["response_correction"]["validation_error"]
                .as_str()
                .unwrap()
                .contains("mandatory reconstruction plan")
        );
        assert_eq!(saved["response_correction"]["attempt"], attempt);
        assert_eq!(saved["admitted_work"], program["admitted_work"]);
        assert_eq!(saved["endpoint_search"], program["endpoint_search"]);
        fields["program_json"] = rejected["callback_params"]["program_json"].clone();
    }
    let stopped = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
    assert_eq!(stopped["callback_action"], "Fail");
    assert!(stopped["callback_params"].get("snapshot_json").is_none());
}

#[tokio::test]
#[ignore = "Requires frozen pass16 checkpoints and rebuilt step/expand WASMs"]
async fn pass16_complete_pair_and_fixed_capacity_use_native_callbacks() {
    let dir = std::env::var("FORESIGHT_PASS16_DIR").unwrap();
    let engine = WasmEngine::new().unwrap();
    for topic in ["games", "food"] {
        let record: Value = serde_json::from_slice(&std::fs::read(format!("{dir}/{topic}.json")).unwrap()).unwrap();
        let mut fields = record["fields"].clone();
        let before: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
        fields["transition_count"] = record["counters"]["transition_count"].clone();
        // Offline replay uses the captured elapsed interval against this test's
        // clock; no live run clock is read, reset or resumed.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        fields["started_at_ms"] = json!(now.saturating_sub(1_660_000).to_string());
        let decision = invoke(&engine, "semantic_step", &fields, &json!({})).await;
        assert_eq!(decision["callback_action"], "Reason", "{topic}: {decision}");
        assert_eq!(decision["callback_params"]["phase"], "compose");
        assert!(decision["callback_params"].get("started_at_ms").is_none());
        let after: Value = serde_json::from_str(decision["callback_params"]["program_json"].as_str().unwrap()).unwrap();
        for key in ["endpoint_search", "results", "evaluations"] { assert_eq!(after[key],before[key],"{topic}: {key}"); }
        // Replay the captured reply as a first capacity rejection: irreducible
        // cost must stop immediately, not spend two more correction sessions.
        let mut first_attempt = before.clone();
        first_attempt.as_object_mut().unwrap().remove("response_correction");
        fields["program_json"] = json!(first_attempt.to_string());
        let rejected = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
        assert_eq!(rejected["callback_action"], "Fail", "{topic}: {rejected}");
        let error = rejected["callback_params"]["error_message"].as_str().unwrap();
        assert!(error.contains("existing graph alone"),"{error}");
        assert!(error.contains("Further shrink corrections were not requested"));
        assert!(rejected["callback_params"].get("snapshot_json").is_none());
        assert!(rejected["callback_params"].get("program_json").is_none());
        assert!(rejected["callback_params"].get("started_at_ms").is_none());
    }
}

#[tokio::test]
#[ignore = "Requires explicitly generated synthetic answer-boundary fixture"]
async fn targeted_repair_native_no_change_and_failure_preserve_checkpoint() {
    let path=std::env::var("FORESIGHT_REPAIR_BOUNDARY_FIXTURE").unwrap();
    let fixture:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut program=fixture["program"].clone();
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    // Rebase only this synthetic test clock; source/program/trace remain exact.
    program["answer_checkpoint"]["started_at_ms"]=json!(now.saturating_sub(1000).to_string());
    let checkpoint=program["answer_checkpoint"].clone();
    let mut fields=json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":program.to_string(),"trace_json":"[]","answer":fixture["answer"].to_string(),"phase":"backward","transition_count":21,"started_at_ms":now.saturating_sub(1000).to_string()});
    let engine=WasmEngine::new().unwrap();
    let next=invoke(&engine,"semantic_step",&fields,&json!({})).await;
    assert_eq!(next["callback_action"],"Reason","{next}");
    assert_eq!(next["callback_params"]["phase"],"backward");
    assert!(next["callback_params"].get("started_at_ms").is_none());
    fields["program_json"]=next["callback_params"]["program_json"].clone();
    if let Ok(path)=std::env::var("FORESIGHT_CHECKPOINT_FIXTURE") { std::fs::write(path,json!({"entity_id":"synthetic-repair-boundary","status":"Reasoning","fields":fields,"provenance":"Actual WASM callback over a synthetic already-validated-answer boundary; not a live research result"}).to_string()).unwrap(); }
    let draft=json!({"hypotheses":[],"branches":[],"routes":[],"research_evidence":[],"amendments":[],"repair_disposition":{"input_fingerprint":program["targeted_repair"]["obligation"]["input_fingerprint"],"status":"no_change","note":"The available evidence does not resolve the recorded bridge."}});
    fields["reasoning_result"]=json!(draft.to_string());
    let complete=invoke(&engine,"semantic_expand",&fields,&json!({})).await;
    assert_eq!(complete["callback_action"],"Complete","{complete}");
    assert_eq!(complete["callback_params"]["answer"],fields["answer"]);
    let saved:Value=serde_json::from_str(complete["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(saved["answer_checkpoint"],checkpoint);
    assert_eq!(saved["targeted_repair"]["status"],"no_change");
    assert!(complete["callback_params"].get("started_at_ms").is_none());
    let mut invalid=draft;invalid["repair_disposition"]["input_fingerprint"]=json!("wrong-input");
    fields["reasoning_result"]=json!(invalid.to_string());
    let failed=invoke(&engine,"semantic_expand",&fields,&json!({})).await;
    assert_eq!(failed["callback_action"],"Fail");
    assert!(failed["callback_params"].get("answer").is_none());
    assert!(failed["callback_params"].get("program_json").is_none());
    assert_eq!(serde_json::from_str::<Value>(fields["program_json"].as_str().unwrap()).unwrap()["answer_checkpoint"],checkpoint);
}

#[tokio::test]
#[ignore = "Requires authorized captured pass17 food checkpoint"]
async fn captured_pass17_blocking_error_cannot_complete_unperformed_comparisons() {
    let capture: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_PASS17_FOOD").unwrap()).unwrap()).unwrap();
    let engine = WasmEngine::new().unwrap();
    let mut fields = capture["fields"].clone();
    let mut program: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    // Offline replay with a synthetic live clock; all graph, error and provider
    // receipts remain the actual captured values. No acceptance run is resumed.
    fields["started_at_ms"] = json!((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 - 1_800_000).to_string());
    let blocked = invoke(&engine,"semantic_step",&fields,&json!({})).await;
    assert_eq!(blocked["callback_action"],"Fail","{blocked}");
    assert!(blocked["callback_params"]["error_message"].as_str().unwrap().contains("max_tokens_exceeded"));
    assert!(blocked["callback_params"].get("started_at_ms").is_none());
    program["stage"] = json!("proposals");
    program["endpoint_proposal_attempt"]["status"] = json!("checking");
    program["deferred_novelty_recheck"]["status"] = json!("checking");
    program["tasks"] = program["endpoint_proposal_attempt"]["tasks"].clone();
    program["cursor"] = json!(0);
    fields["program_json"] = json!(program.to_string());
    let interrupted=invoke(&engine,"semantic_step",&fields,&json!({})).await;
    assert_eq!(interrupted["callback_action"],"SearchPlanned","{interrupted}");
    let saved:Value=serde_json::from_str(interrupted["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(saved["deferred_novelty_recheck"]["status"],"interrupted");
    assert_eq!(saved["deferred_novelty_recheck"]["performed_checks"],0);
    assert_eq!(saved["deferred_novelty_recheck"]["pending_checks"],5);
    assert_eq!(saved["endpoint_search"],program["endpoint_search"]);
    assert_eq!(saved["baseline"],program["baseline"]);
    for (id,receipt) in program["endpoint_novelty"].as_object().unwrap() {
        assert_eq!(saved["endpoint_novelty"][id]["initial_check"],receipt["initial_check"]);
    }
    fields["program_json"]=interrupted["callback_params"]["program_json"].clone();
    if let Ok(path)=std::env::var("FORESIGHT_INTERRUPTED_FIXTURE") {
        let mut envelope=capture.clone();
        envelope["fields"]=fields.clone();
        envelope["status"]=json!("Choosing");
        envelope["provenance"]=json!("Captured pass17 inputs replayed through actual WASM with a synthetic live clock; no live run resumed");
        std::fs::write(path,envelope.to_string()).unwrap();
    }
    let terminal=invoke(&engine,"semantic_step",&fields,&json!({})).await;
    assert_eq!(terminal["callback_action"],"Fail");
    assert!(terminal["callback_params"]["error_message"].as_str().unwrap().contains("max_tokens_exceeded"));
}

#[tokio::test]
#[ignore = "Requires actual-WASM context-limit producer over captured input"]
async fn context_limit_audit_preserves_unperformed_receipt() {
    let mut envelope:Value=serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_CONTEXT_LIMIT_PRODUCER").unwrap()).unwrap()).unwrap();
    let mut p:Value=serde_json::from_str(envelope["fields"]["program_json"].as_str().unwrap()).unwrap();
    p["stage"]=json!("routes");p["tasks"]=json!([]);p["cursor"]=json!(0);p["stop_reason"]=json!("");
    envelope["fields"]["program_json"]=json!(p.to_string());
    envelope["fields"]["started_at_ms"]=json!((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64-1_800_000).to_string());
    let engine=WasmEngine::new().unwrap();
    let out=invoke(&engine,"semantic_step",&envelope["fields"],&json!({})).await;
    let encoded=out["callback_params"]["program_json"].as_str().unwrap_or_else(||panic!("{out}"));
    let program:Value=serde_json::from_str(encoded).unwrap();
    let routes=program["endpoint_search"]["routes"].as_array().unwrap();
    let limited:Vec<_>=routes.iter().filter(|r|r["audit"]["context_limited_checks"].as_u64().unwrap_or(0)>0).collect();
    assert!(!limited.is_empty());
    for route in limited {
        assert_ne!(route["status"],"checked");
        for check in route["audit"]["checks"].as_array().unwrap().iter().filter(|c|c["execution"].is_object()) {
            assert!(check["result"].is_null()&&check["probability"].is_null());
        }
    }
    envelope["fields"]["program_json"]=json!(encoded);
    if let Ok(path)=std::env::var("FORESIGHT_CONTEXT_LIMIT_AUDIT_PRODUCER") {std::fs::write(path,envelope.to_string()).unwrap();}
}

#[tokio::test]
#[ignore = "Requires captured-context synthetic append-only patch fixture and rebuilt expand"]
async fn captured_patch_receiver_preserves_context_and_enforces_exact_capacity() {
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("FORESIGHT_PATCH_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let mut program = fixture["program"].clone();
    program["transition_count"] = json!(40);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut fields = json!({"snapshot_json":fixture["snapshot"].to_string(),"program_json":program.to_string(),"trace_json":"[]","phase":"backward","reasoning_result":fixture["generated"].to_string(),"transition_count":40,"started_at_ms":now.saturating_sub(1000).to_string()});
    let engine = WasmEngine::new().unwrap();
    let accepted = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
    let diagnostic = accepted["callback_params"]["program_json"]
        .as_str()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .map(|p| p["response_correction"]["validation_error"].clone());
    assert_eq!(
        accepted["callback_action"], "Expanded",
        "{:?} {}",
        diagnostic, accepted["callback_params"]["error_message"]
    );
    let after: Value = serde_json::from_str(
        accepted["callback_params"]["snapshot_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    for node in fixture["snapshot"]["nodes"].as_array().unwrap() {
        assert!(after["nodes"].as_array().unwrap().contains(node));
    }
    let checked: Value = serde_json::from_str(
        accepted["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let required = checked["admitted_work"]["actual_mandatory_transitions"]
        .as_u64()
        .unwrap();
    assert!(required > 0);
    program["admitted_work"]["evaluation_transition_capacity"] = json!(required - 1);
    fields["program_json"] = json!(program.to_string());
    let rejected = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
    assert_eq!(
        rejected["callback_action"], "CompositionRejected",
        "{}",
        rejected["callback_params"]["error_message"]
    );
    assert!(rejected["callback_params"].get("snapshot_json").is_none());
    assert!(rejected["callback_params"].get("started_at_ms").is_none());
    let mut changed = fixture["generated"].clone();
    changed["baseline_delta"] = json!({"unknowns":["Changed"]});
    fields["reasoning_result"] = json!(changed.to_string());
    let forbidden = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
    assert_eq!(forbidden["callback_action"], "CompositionRejected");
    let receipt: Value = serde_json::from_str(
        forbidden["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        receipt["response_correction"]["validation_error"]
            .as_str()
            .unwrap()
            .contains("omit baseline_delta")
    );
    program["admitted_work"]["evaluation_transition_capacity"] = json!(200);
    program["admitted_work"]["finalization_time_reserve_ms"] = json!(1_400_000);
    fields["program_json"] = json!(program.to_string());
    fields["reasoning_result"] = json!(fixture["generated"].to_string());
    fields["started_at_ms"] = json!(now.saturating_sub(2_200_000).to_string());
    let late = invoke(&engine, "semantic_expand", &fields, &json!({})).await;
    assert_eq!(late["callback_action"], "Fail");
    assert!(
        late["callback_params"]["error_message"]
            .as_str()
            .unwrap()
            .contains("insufficient original time")
    );
    assert!(late["callback_params"].get("snapshot_json").is_none());
}

#[tokio::test]
#[ignore = "requires authorized pass20 captured candidate/evidence context"]
async fn captured_retrieval_v2_crosses_native_receiver_boundary() {
    let capture: Value = serde_json::from_slice(&std::fs::read(std::env::var("FORESIGHT_RETRIEVAL_NATIVE_CAPTURE").unwrap()).unwrap()).unwrap();
    let mut fields = capture["fields"].clone();
    let mut program: Value = serde_json::from_str(fields["program_json"].as_str().unwrap()).unwrap();
    // Reconstructed receiver input, not replay of an expired run: retained
    // candidate/evidence values, no new provider call or claimed live retrieval.
    program["proposal_pool"]["stage"] = json!("contrast");
    program["proposal_pool"]["retrieval_contract"] = json!(2);
    for key in ["response_correction", "stop_reason", "scope_repair"] { program.as_object_mut().unwrap().remove(key); }
    let mut rows=Vec::new();
    for endpoint in program["proposal_pool"]["candidates"].as_array().unwrap() {
        let mut contrast=endpoint["contrast"].clone();
        let refs=contrast["present_analogue"]["evidence_ids"].clone();
        let challenge=&mut contrast["frontier_challenge"];
        for key in ["reported_queries","query_provenance","retrieval_provenance"] { challenge.as_object_mut().unwrap().remove(key); }
        challenge["retrieval_contract"]=json!(2);
        challenge["retrieval_reports"]=json!([{"mode":"direct_fetch","evidence_ids":refs}]);
        rows.push(json!({"endpoint_id":endpoint["id"],"contrast":contrast}));
    }
    let generated=json!({"hypotheses":[],"research_evidence":[],"continue_exploring":false,"exploration_note":"Offline reconstructed retrieval boundary", "proposal_contrasts":rows,"comparison_priority":rows.iter().map(|r|r["endpoint_id"].clone()).collect::<Vec<_>>()});
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis().to_string();
    fields["phase"]=json!("explore");fields["started_at_ms"]=json!(now);
    fields["program_json"]=json!(program.to_string());fields["reasoning_result"]=json!(generated.to_string());
    let engine=WasmEngine::new().unwrap();
    let accepted=invoke(&engine,"semantic_expand",&fields,&json!({})).await;
    assert_eq!(accepted["callback_action"],"Expanded", "{}", accepted["callback_params"]["program_json"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()).map(|p|p["response_correction"]["validation_error"].clone()).unwrap_or_default());
    let after:Value=serde_json::from_str(accepted["callback_params"]["program_json"].as_str().unwrap()).unwrap();
    assert_eq!(after["proposal_pool"]["candidates"].as_array().unwrap().len(),rows.len());
    for (endpoint,row) in after["proposal_pool"]["candidates"].as_array().unwrap().iter().zip(&rows) {
        assert_eq!(endpoint["contrast"]["frontier_challenge"]["retrieval_reports"],row["contrast"]["frontier_challenge"]["retrieval_reports"]);
        assert_eq!(endpoint["contrast"]["frontier_challenge"]["retrieval_provenance"],"researcher_report_not_verified_against_tool_trace");
    }
    for (report,message) in [(json!({"mode":"direct_fetch","evidence_ids":["missing-finding"]}),"active present findings"),(json!({"mode":"invented-mode","evidence_ids":[]}),"Unknown reported retrieval mode")] {
        let mut rejected=generated.clone();rejected["proposal_contrasts"][0]["contrast"]["frontier_challenge"]["retrieval_reports"]=json!([report]);
        fields["reasoning_result"]=json!(rejected.to_string());
        let result=invoke(&engine,"semantic_expand",&fields,&json!({})).await;
        assert_eq!(result["callback_action"],"CompositionRejected","{result}");
        let preserved:Value=serde_json::from_str(result["callback_params"]["program_json"].as_str().unwrap()).unwrap();
        assert!(preserved["response_correction"]["validation_error"].as_str().unwrap().contains(message));
        assert_eq!(preserved["proposal_pool"]["candidates"],program["proposal_pool"]["candidates"]);
    }
    if let Ok(path)=std::env::var("FORESIGHT_RETRIEVAL_NATIVE_OUTPUT") {
        fields["program_json"]=accepted["callback_params"]["program_json"].clone();
        fields["snapshot_json"]=accepted["callback_params"]["snapshot_json"].clone();
        fields["answer"]=json!("");
        std::fs::write(path,json!({"entity_id":"reconstructed-retrieval-v2-boundary","status":"Choosing","fields":fields,"provenance":"Actual WASM receiver over reconstructed captured pass20 context. Direct-fetch reports are explicit test transformations; not live retrieval verification or expired-run replay."}).to_string()).unwrap();
    }
}
