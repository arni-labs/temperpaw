//! Offline reconstruction of a captured graph before challenge; generated consequences are synthetic.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmResourceLimits,
};

fn bytes(module: &str) -> Vec<u8> {
    let key = if module == "semantic_reasoning" {
        "ROLLOUT_REASONING_WASM"
    } else {
        "ROLLOUT_EXPAND_WASM"
    };
    let path=std::env::var(key).unwrap_or_else(|_|format!("{}/../../os-apps/paw-foresight/wasm/{module}/target/wasm32-unknown-unknown/release/{module}.wasm",env!("CARGO_MANIFEST_DIR")));
    std::fs::read(path).unwrap()
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
        ctx,
        Arc::new(SimWasmHost::new().with_default_response(500, "unexpected provider IO")),
        hash,
    )
    .await
}
async fn invoke_with_host(
    engine: &WasmEngine,
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

#[allow(dead_code)]
mod core {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../os-apps/paw-foresight/wasm/semantic_core.rs"
    ));
}
#[tokio::test]
#[ignore = "Captured free-time fixture supplied explicitly; response is synthetic"]
async fn challenge_producer_binds_both_signs_and_preserves_joint_semantics() {
    let c: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("ROLLOUT_CHECKPOINT").unwrap()).unwrap(),
    )
    .unwrap();
    let mut snapshot = c["snapshot"].clone();
    snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|n| n["kind"] != "world" && !n["Id"].as_str().unwrap().starts_with("r4-"));
    let mut p = c["program"].clone();
    p["round"] = json!(3);
    p["independent_challenge"] = json!({"status":"pending"});
    p["stage"] = json!("exploration");
    let fields = json!({"snapshot_json":snapshot.to_string(),"program_json":p.to_string(),"phase":"challenge","started_at_ms":c["started_at_ms"].as_str().map(str::to_owned).unwrap_or_else(||c["started_at_ms"].to_string())});
    let engine = WasmEngine::new().unwrap();
    let launch = invoke(&engine, "semantic_reasoning", fields.clone()).await;
    assert_eq!(launch["callback_action"], "LaunchReasoning");
    let input: Value =
        serde_json::from_str(launch["callback_params"]["user_message"].as_str().unwrap()).unwrap();
    assert!(input.get("causal_rollout").is_none());
    // The test response chooses its own premise; production supplies no roots.
    let premise = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| core::branches::future_eligible(&snapshot, &p, core::field(n, "Id")))
        .unwrap();
    let parent = premise.get("branch_id").cloned().unwrap_or(Value::Null);
    let rollout = json!({"nodeId":premise["Id"],"statement":premise["statement"],"branches":[
        {"id":"challenge-premise-on","parent_branch_id":parent,"condition":{"kind":"all_occurring","event_ids":[premise["Id"]]},"by":snapshot["world"]["target_date"]},
        {"id":"challenge-premise-off","parent_branch_id":parent,"condition":{"kind":"not_all_occurring","event_ids":[premise["Id"]]},"by":snapshot["world"]["target_date"]}
    ]});
    let generated = json!({"branches":rollout["branches"],"hypotheses":[{"id":"on_effect","statement":"A downstream consequence within the question horizon","requires":[],"branch_id":"challenge-premise-on","mechanism":"The selected event changes available choices"},{"id":"off_effect","statement":"A different consequence within the question horizon","requires":[],"branch_id":"challenge-premise-off","mechanism":"Failure of the selected event leaves different available choices"}],"research_evidence":[],"premises_challenged":[{"assumption":rollout["statement"],"alternative":"Investigate both exact signs without asserting a particular opposite","prior_hypothesis_ids":[rollout["nodeId"]],"alternative_hypothesis_ids":["on_effect","off_effect"]}],"continue_exploring":true,"exploration_note":"Synthetic contract proof, not evidence of output quality"});
    let mut f = fields.clone();
    f["reasoning_result"] = json!(generated.to_string());
    let expanded = invoke(&engine, "semantic_expand", f).await;
    assert_eq!(expanded["callback_action"], "Expanded", "{expanded}");
    let next: Value = serde_json::from_str(
        expanded["callback_params"]["snapshot_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut np: Value = serde_json::from_str(
        expanded["callback_params"]["program_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        expanded["callback_params"]["started_at_ms"],
        fields["started_at_ms"]
    );
    assert_eq!(
        &next["nodes"].as_array().unwrap()[..snapshot["nodes"].as_array().unwrap().len()],
        snapshot["nodes"].as_array().unwrap()
    );
    assert_eq!(np["evidence_ids"], p["evidence_ids"]);
    assert!(np["independent_challenge"].get("causal_rollout").is_none());
    assert_eq!(
        np["independent_challenge"]["branches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for (id, kind) in [
        ("r4-on_effect", "all_occurring"),
        ("r4-off_effect", "not_all_occurring"),
    ] {
        assert!(
            np["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["nodeId"] == id && t["function"] == "estimate_conditional")
        );
        np["results"][id]["classify_temporal"] = json!("future_change");
        let mut reqp = np.clone();
        reqp["tasks"] = json!([{"nodeId":id,"function":"estimate_conditional","depth":0}]);
        reqp["cursor"] = json!(0);
        let req = core::request(&next, &reqp).unwrap();
        let conditions = &req["state"]["branch_state"]["conditions"];
        assert_eq!(conditions.as_array().unwrap().last().unwrap()["kind"], kind);
        let wc = core::branches::world_conditions(&next, &json!([id])).unwrap();
        assert_eq!(wc.as_array().unwrap().last().unwrap()["kind"], kind);
    }
    assert!(
        core::branches::world_conditions(&next, &json!(["r4-on_effect", "r4-off_effect"])).is_err()
    );
    let mut cost_program = np.clone();
    cost_program["tasks"] = json!(
        np["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| matches!(t["nodeId"].as_str(), Some("r4-on_effect" | "r4-off_effect")))
            .cloned()
            .collect::<Vec<_>>()
    );
    fn batches(snapshot: &Value, program: &Value) -> usize {
        let mut p = program.clone();
        let mut cursor = 0;
        let mut count = 0;
        while cursor < p["tasks"].as_array().unwrap().len() {
            p["cursor"] = json!(cursor);
            let batch = core::batch::prepare(snapshot, &p, 16).unwrap();
            cursor += batch.tasks.len();
            count += 1;
        }
        count
    }
    let new_batches = batches(&next, &cost_program);
    let mut old_snapshot = next.clone();
    for n in old_snapshot["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|n| matches!(n["Id"].as_str(), Some("r4-on_effect" | "r4-off_effect")))
    {
        n.as_object_mut().unwrap().remove("branch_id");
        n.as_object_mut().unwrap().remove("branch_state");
    }
    let mut old_program = cost_program.clone();
    old_program["tasks"]
        .as_array_mut()
        .unwrap()
        .retain(|t| t["function"] != "estimate_conditional");
    let old_batches = batches(&old_snapshot, &old_program);
    println!(
        "Matched synthetic two-consequence plan: old {}tasks/{}HTTP/{}transitions incl same reasoning reserve; new {}tasks/{}HTTP/{}transitions",
        old_program["tasks"].as_array().unwrap().len(),
        old_batches,
        core::REASONING_ADMISSION_RESERVE + 2 * old_batches as u64,
        cost_program["tasks"].as_array().unwrap().len(),
        new_batches,
        core::REASONING_ADMISSION_RESERVE + 2 * new_batches as u64
    );
    let mut bad = generated.clone();
    bad["hypotheses"][1]["branch_id"] = json!("missing-branch");
    let mut f = fields;
    f["reasoning_result"] = json!(bad.to_string());
    let reject = invoke(&engine, "semantic_expand", f).await;
    assert_eq!(reject["callback_action"], "CompositionRejected");
    assert!(reject["callback_params"]["snapshot_json"].is_null());
}
