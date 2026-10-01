//! Native research/deepening boundaries, without any real provider calls.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
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
    let r = engine
        .invoke(
            &hash,
            &ctx,
            Arc::new(SimWasmHost::new().with_default_response(500, "unexpected provider IO")),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    serde_json::to_value(r).unwrap()
}
#[tokio::test]
async fn research_recommendation_explores_without_resetting_global_budget() {
    let engine = WasmEngine::new().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let snapshot = json!({"world":{"target_date":"2027-09-19","hindcast_mode":"false"},"nodes":[{"Id":"e","edges":"[]"},{"Id":"s","kind":"scenario","edges":"[{\"kind\":\"requires\",\"to_id\":\"e\"}]"}]});
    let program = json!({"cursor":0,"tasks":[],"results":{"s":{"classify_gap":"evidence","choose_next_operation":"research"},"e":{"classify_gap":"uncertain","choose_next_operation":"research"}},"issues":[]});
    let mut fields = json!({"phase":"seed","snapshot_json":snapshot.to_string(),"program_json":program.to_string(),"trace_json":"[]","started_at_ms":now.to_string(),"reasoning_session_id":"research-session"});
    let step = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(step["callback_action"], "Reason", "{step}");
    assert_eq!(step["callback_params"]["phase"], "explore");
    fields["phase"] = json!("explore");
    fields["program_json"] = step["callback_params"]["program_json"].clone();
    let reasoning = invoke(&engine, "semantic_reasoning", fields.clone()).await;
    assert_eq!(
        reasoning["callback_action"], "LaunchReasoning",
        "{reasoning}"
    );
    assert_eq!(
        reasoning["callback_params"]["tools_enabled"],
        "temper_web_search,temper_web_fetch"
    );
    assert_eq!(reasoning["callback_params"]["max_turns"], "32");
    assert_eq!(reasoning["callback_params"]["tool_choice"], "auto");

    fields["reasoning_result"]=json!(json!({"continue_exploring":true,"exploration_note":"Investigate an alternate mechanism","research_evidence":[],"hypotheses":[{"id":"r1","parent":"s","statement":"A revised hypothetical mechanism","requires":["e"],"research_question":"What adoption evidence is still missing?"}]}).to_string());
    let expanded = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(expanded["callback_action"], "Expanded", "{expanded}");
    let saved: Value = serde_json::from_str(
        expanded["callback_params"]["snapshot_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["nodes"][1], snapshot["nodes"][1]);

    fields["snapshot_json"] = expanded["callback_params"]["snapshot_json"].clone();
    fields["program_json"] = expanded["callback_params"]["program_json"].clone();
    let recheck = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(
        recheck["callback_action"], "Evaluate",
        "new hypotheses require evaluation: {recheck}"
    );
    let request: Value =
        serde_json::from_str(recheck["callback_params"]["request_json"].as_str().unwrap()).unwrap();
    assert_eq!(request["state"]["node"]["Id"], "r1-r1");
    assert_eq!(
        request["state"]["prerequisites"][0]["assessment"]["classify_gap"],
        "uncertain"
    );
    fields["started_at_ms"] = json!((now - 3_600_001).to_string());
    let exhausted = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(exhausted["callback_params"]["phase"], "synthesize");
    let program: Value = serde_json::from_str(
        exhausted["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(program["stop_reason"], "time_budget");
    assert_eq!(program["remaining_calls"], 5000);
}

#[tokio::test]
async fn synthesis_uses_jev_event_estimates_instead_of_invented_weights() {
    let engine = WasmEngine::new().unwrap();
    let snapshot = json!({"world":{"target_date":"2027"},"nodes":[{"Id":"h1","kind":"scenario"},{"Id":"h2","kind":"revision"},{"Id":"support","kind":"evidence"}]});
    let outcome = |id: &str| json!({"id":id,"hypothesis_id":id,"title":"Future","definition":"Event by horizon","narrative":"Mechanism","scene":"September 2027: a builder ships a clinic booking fix before lunch.","what_you_can_do":["Watch one clinic handle a missed booking."],"probability":0.99,"scenario_ids":["h1","support"],"signals":["Signal"],"falsifiers":["Falsifier"]});
    let answer = json!({"schema":"foresight-outlook-v2","probability_model":"overlapping_events","probability_basis":"model_implied_event_estimate","calibrated":false,"headline":"Independent events","summary":"They can coexist","horizon":"2027","evidence_limits":["Uncalibrated"],"research_questions":[],"outcomes":[outcome("h1"),outcome("h2")]});
    let mut fields = json!({"phase":"synthesize","snapshot_json":snapshot.to_string(),"program_json":json!({"results":{"h1":{"estimate_likelihood":"0.8"},"h2":{"estimate_likelihood":"0.7"}}}).to_string(),"reasoning_result":answer.to_string()});
    let good = invoke(&engine, "semantic_expand", fields.clone()).await;
    assert_eq!(good["callback_action"], "Complete", "{good}");
    let saved: Value =
        serde_json::from_str(good["callback_params"]["answer"].as_str().unwrap()).unwrap();
    assert_eq!(saved["outcomes"][0]["probability"], 0.8);
    assert_eq!(saved["outcomes"][1]["probability"], 0.7);
    assert_eq!(
        saved["outcomes"][0]["scenario_ids"],
        json!(["h1", "support"])
    );
    assert_eq!(
        saved["outcomes"][0]["scene"],
        answer["outcomes"][0]["scene"]
    );
    assert_eq!(
        saved["outcomes"][0]["what_you_can_do"],
        answer["outcomes"][0]["what_you_can_do"]
    );
    let mut invalid_scene = answer.clone();
    invalid_scene["outcomes"][0]["scene"] = json!("x".repeat(601));
    fields["reasoning_result"] = json!(invalid_scene.to_string());
    assert_eq!(
        invoke(&engine, "semantic_expand", fields.clone()).await["callback_action"],
        "Fail"
    );

    let mut bad = answer;
    bad["outcomes"][0]["hypothesis_id"] = json!("invented");
    fields["reasoning_result"] = json!(bad.to_string());
    assert_eq!(
        invoke(&engine, "semantic_expand", fields).await["callback_action"],
        "Fail"
    );
}

#[tokio::test]
async fn provider_402_without_estimates_fails_before_synthesis() {
    let engine = WasmEngine::new().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let p = json!({"cursor":0,"tasks":[{"nodeId":"h","function":"classify_gap"}],"stop_reason":"provider_error","last_error":"Semantic provider HTTP 402","results":{}});
    let mut fields = json!({"program_json":p.to_string(),"snapshot_json":json!({"nodes":[{"Id":"h","kind":"scenario"}]}).to_string(),"trace_json":"[{\"error\":\"Semantic provider HTTP 402\"}]","started_at_ms":now.to_string()});
    let failed = invoke(&engine, "semantic_step", fields.clone()).await;
    assert_eq!(failed["callback_action"], "Fail", "{failed}");
    assert_eq!(
        failed["callback_params"]["error_message"],
        "Semantic provider HTTP 402"
    );
    let mut partial = p;
    partial["results"]["h"] = json!({"estimate_likelihood":"0.37"});
    fields["program_json"] = json!(partial.to_string());
    let preserved = invoke(&engine, "semantic_step", fields).await;
    assert_eq!(preserved["callback_action"], "Reason");
    assert_eq!(preserved["callback_params"]["phase"], "synthesize");
}
