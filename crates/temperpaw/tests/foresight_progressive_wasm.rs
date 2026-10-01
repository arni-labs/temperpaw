//! Real deterministic modules at the host boundary: no provider output is substituted.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

async fn aggregate(action: &str, fields: Value, host: SimWasmHost) -> Value {
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let dir =
        PathBuf::from(std::env::var_os("FORESIGHT_WASM_DIR").expect("set verified WASM directory"));
    let bytes = std::fs::read(dir.join("aggregate_costs.wasm")).unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let ctx = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "World".into(),
        entity_id: "world-1".into(),
        trigger_action: action.into(),
        wasm_module: Some("aggregate_costs".into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"Active","fields":fields}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "temper_api_url".into(),
            "https://temper.test".into(),
        )]),
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
            Arc::new(host),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    serde_json::to_value(result).unwrap()
}

#[tokio::test]
async fn first_pass_scores_objections_while_deeper_pass_can_revise() {
    for (phase, expected) in [("first_pass", "Score"), ("deepening", "RevisionRequested")] {
        let host = SimWasmHost::new()
            .with_default_response(404, "unexpected read")
            .with_response(
                "https://temper.test/tdata/Worlds('world-1')",
                200,
                &json!({"fields":{"exploration_phase":phase}}).to_string(),
            );
        let result = aggregate("EvaluateChallenge",json!({"world_id":"world-1","claim_id":"claim-1",
            "round_count":"0","cost_flags":"[]","challenge_flags":[{"kind":"miracle","severity":"high"}]}),host).await;
        assert_eq!(result["callback_action"], expected, "{result}");
        if phase == "first_pass" {
            assert_eq!(result["callback_params"]["repair_cost"], "80.00");
        }
    }
}

#[tokio::test]
async fn background_plan_uses_terminal_claims_and_fences_each_step() {
    let host = SimWasmHost::new()
        .with_default_response(404, "unexpected read")
        .with_response(
            "https://temper.test/tdata/Claims?$filter=world_id eq 'world-1'",
            200,
            r#"{"value":[{"entity_id":"c-2"},{"entity_id":"c-1"}]}"#,
        )
        .with_response(
            "https://temper.test/tdata/Claims('c-1')",
            200,
            r#"{"status":"Settled"}"#,
        )
        .with_response(
            "https://temper.test/tdata/Claims('c-2')",
            200,
            r#"{"status":"Unreachable"}"#,
        );
    let plan = aggregate(
        "MaybeDeepen",
        json!({"exploration_phase":"first_pass"}),
        host,
    )
    .await;
    assert_eq!(plan["callback_action"], "BeginDeepening", "{plan}");
    let mut fields = plan["callback_params"].clone();
    for (cursor, id) in [(0, "c-1"), (1, "c-2")] {
        let result = aggregate("ContinueDeepening", fields.clone(), SimWasmHost::new()).await;
        assert_eq!(
            result["callback_action"], "DeepeningClaimPrepared",
            "{result}"
        );
        assert_eq!(result["callback_params"]["deepening_claim_id"], id);
        assert_eq!(
            result["callback_params"]["expected_deepening_cursor"],
            cursor.to_string()
        );
        fields["deepening_cursor"] = result["callback_params"]["deepening_cursor"].clone();
    }
    let done = aggregate("ContinueDeepening", fields, SimWasmHost::new()).await;
    assert!(
        done["callback_action"].as_str().unwrap_or("").is_empty(),
        "{done}"
    );
}

#[tokio::test]
async fn first_pass_waits_for_every_endpoint_to_attach_its_claims() {
    for (attached, listed, expect_completion) in [
        (false, false, false),
        (true, false, false),
        (true, true, true),
    ] {
        let mut claims = vec![
            json!({"entity_id":"c-1","status":"Settled","fields":{"endpoint_id":"e-1","best_route_cost":"40","settled_route_id":"p-1"}}),
        ];
        if listed {
            claims.push(json!({"entity_id":"c-2","status":"Settled","fields":{"endpoint_id":"e-2","best_route_cost":"60","settled_route_id":"p-2"}}));
        }
        let host = SimWasmHost::new()
            .with_default_response(200, "{}")
            .with_response("https://temper.test/tdata/Claims?$filter=world_id eq 'world-1'", 200,
                &json!({"value":claims}).to_string())
            .with_response("https://temper.test/tdata/Worlds('world-1')", 200,
                r#"{"fields":{"exploration_phase":"first_pass","first_pass_complete":false,"endpoint_budget":"2"}}"#)
            .with_response("https://temper.test/tdata/Endpoints?$filter=world_id eq 'world-1'", 200,
                r#"{"value":[{"entity_id":"e-1"},{"entity_id":"e-2"}]}"#)
            .with_response("https://temper.test/tdata/Endpoints('e-1')", 200,
                r#"{"status":"UnderRepair","fields":{"claim_ids":"[\"c-1\"]"}}"#)
            .with_response("https://temper.test/tdata/Endpoints('e-2')", 200,
                &json!({"status":"UnderRepair","fields":{"claim_ids":if attached {"[\"c-2\"]"} else {"[]"}}}).to_string())
            // Deliberate sentinel: an attempted completion is observable in the
            // result instead of a mock silently accepting an unexpected write.
            .with_response("https://temper.test/tdata/Worlds('world-1')/TemperPaw.PathsScored", 409,
                "completion-observed");
        let result = aggregate(
            "EvaluateWorldCascade",
            json!({"exploration_phase":"first_pass"}),
            host,
        )
        .await;
        assert_eq!(
            result["success"], !expect_completion,
            "attached={attached}, listed={listed}: {result}"
        );
        if expect_completion {
            assert!(
                result["error"]
                    .as_str()
                    .unwrap()
                    .contains("completion-observed"),
                "{result}"
            );
        }
    }
}
