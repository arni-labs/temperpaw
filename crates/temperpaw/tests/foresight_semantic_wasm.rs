use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
};
use temper_jit::TransitionTable;
use temper_runtime::scheduler::SimActorHandler;
use temper_server::entity_actor::sim_handler::EntityActorHandler;
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

fn module_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new("bash")
            .current_dir(&root)
            .args(["-c", "set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-foresight/wasm/evaluate_semantics wasm32-unknown-unknown --locked"])
            .output().expect("build evaluation WASM");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        std::fs::read(String::from_utf8(output.stdout).unwrap().trim()).unwrap()
    })
}

fn packet_fixture(mode: &str) -> (String, String) {
    use sha2::{Digest, Sha256};
    let questions: Value = serde_json::from_str(include_str!(
        "../../../os-apps/paw-foresight/wasm/evaluation_contract.json"
    ))
    .unwrap();
    let packet=json!({"schema_version":"foresight-evaluation-packet-v2","mode":mode,"questions":questions,"state":{"world_id":"w-1","path_id":"p-1","repair":"A requires B. B occurs first.","endpoint_bundle":"A reaches production after B.","observed_graph":"B already exists.","required_nodes":[]}}).to_string();
    let hash = format!("{:x}", Sha256::digest(packet.as_bytes()));
    (packet, hash)
}

#[tokio::test(flavor = "multi_thread")]
async fn declared_spawn_reaches_terminal_callback_through_real_runtime() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use temper_authz::AuthenticatedRequestContext;
    use temper_runtime::{ActorSystem, tenant::TenantId};
    use temper_server::{
        registry::{EntityVerificationResult, SpecRegistry, VerificationStatus},
        request_context::AgentContext,
        state::{DispatchCommand, ServerState},
    };
    use tower::ServiceExt;
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-foresight");
    for (mode, terminal) in [("off", "Skipped"), ("shadow", "Failed")] {
        let xml = std::fs::read_to_string(app.join("specs/model.csdl.xml")).unwrap();
        // Start at the provider boundary and omit only the unrelated critic
        // integration. The production spawn, evaluation, policy and WASM run.
        let mut path = std::fs::read_to_string(app.join("specs/path.ioa.toml"))
            .unwrap()
            .replace("initial = \"Solving\"", "initial = \"Repaired\"")
            .replace(
                "  { type = \"trigger\", name = \"spawn_adversaries\" },\n",
                "",
            );
        let start = path
            .find("[[action.triggers]]\nname = \"spawn_adversaries\"")
            .unwrap();
        let end = start + path[start..].find("[[action]]").unwrap();
        path.replace_range(start..end, "");
        let evaluation = std::fs::read_to_string(app.join("specs/semantic_evaluation.ioa.toml"))
            .unwrap()
            .replace("{secret:temper_api_url}", "http://127.0.0.1:3000");
        let world = format!(
            "[automaton]\nname = \"World\"\nstates = [\"Active\"]\ninitial = \"Active\"\n[[state]]\nname = \"semantic_evaluation_mode\"\ntype = \"string\"\ninitial = \"{mode}\"\n"
        );
        let mut registry = SpecRegistry::new();
        registry.register_tenant(
            "default",
            temper_spec::csdl::parse_csdl(&xml).unwrap(),
            xml,
            &[
                ("Path", &path),
                ("World", &world),
                ("SemanticEvaluation", &evaluation),
            ],
        );
        let tenant = TenantId::default();
        for entity in ["Path", "World", "SemanticEvaluation"] {
            registry.set_verification_status(
                &tenant,
                entity,
                VerificationStatus::Completed(EntityVerificationResult {
                    all_passed: true,
                    levels: vec![],
                    verified_at: "2026-09-18T00:00:00Z".into(),
                }),
            );
        }
        let state = ServerState::from_registry(ActorSystem::new("semantic-spawn"), registry);
        state.rebuild_reaction_dispatcher();
        state
            .authz
            .reload_tenant_policies(
                "default",
                &std::fs::read_to_string(app.join("policies/foresight.cedar")).unwrap(),
            )
            .unwrap();
        let hash = state.wasm_engine.compile_and_cache(module_bytes()).unwrap();
        state
            .wasm_module_registry
            .write()
            .unwrap()
            .register_builtin("evaluate_semantics", &hash);
        state
            .get_or_create_tenant_entity(
                &tenant,
                "World",
                "w-1",
                json!({"semantic_evaluation_mode":mode}),
            )
            .await
            .unwrap();
        state.get_or_create_tenant_entity(&tenant, "Path", "p-1", json!({"world_id":"w-1","endpoint_id":"ep-1","repair_log_file_id":"r-1","required_node_ids":"[]","round_count":"2"})).await.unwrap();
        let system = AgentContext::for_service("system");
        let result = state
            .dispatch(DispatchCommand {
                tenant: &tenant,
                entity_type: "Path",
                entity_id: "p-1",
                action: "ChallengePrepared",
                params: json!({"challenge_packet_json":packet_fixture(mode).0,"challenge_packet_sha256":packet_fixture(mode).1,"expected_repair_log_file_id":"r-1"}),
                agent_ctx: &AgentContext::for_service("wasm-runtime"),
                await_integration: true,
                await_reactions: true,
            })
            .await
            .unwrap();
        assert!(result.success, "{result:?}");
        let mut last = json!(null);
        for _ in 0..100 {
            let mut request = Request::builder()
                .uri("/tdata/SemanticEvaluations")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(AuthenticatedRequestContext::new(
                    tenant.clone(),
                    system.security_ctx.clone().unwrap(),
                ));
            let response = temper_server::build_router(state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert!(response.status().is_success());
            last =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if last["value"]
                .as_array()
                .is_some_and(|rows| rows.len() == 1 && rows[0]["status"] == terminal)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(last["value"][0]["status"], terminal, "{last}");
        let row = &last["value"][0]["fields"];
        assert_eq!(row["parent_id"], "p-1", "{row}");
        assert_eq!(row["round_count"], "2", "{row}");
        if mode == "shadow" {
            assert_eq!(row["error_message"], "missing_typesafe_api_key", "{row}");
        }
    }
}
fn response() -> Value {
    let mut answers = json!({});
    for k in ["contradiction", "incentive", "lag", "miracle"] {
        answers[k] = json!({"type":"choice","choice":"clear","probabilities":{"clear":0.96,"defect":0.02,"unknown":0.02},"confidence":0.9});
    }
    json!({"model":"jev-1.13.0","answers":answers,"usage":{"input_tokens":120,"output_tokens":0}})
}
async fn evaluate(mode: &str, key: bool, provider: Value) -> Value {
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let hash = engine.compile_and_cache(module_bytes()).unwrap();
    let host=SimWasmHost::new().with_default_response(500,"unexpected IO")
 .with_response("https://temper.test/tdata/Worlds('w-1')",200,&json!({"fields":{"semantic_evaluation_mode":mode,"graph_snapshot_file_id":"g-1","last_ingest_date":"2026-09-18","target_date":"2027-09-18"}}).to_string())
 .with_response("https://temper.test/tdata/Files('r-1')/$value",200,"A requires B. B occurs first.")
 .with_response("https://temper.test/tdata/Files('g-1')/$value",200,"Observed evidence: B exists.")
 .with_response("https://api.typesafe.ai/v1/systemone",200,&provider.to_string());
    let mut config = BTreeMap::from([("temper_api_url".into(), "https://temper.test".into())]);
    if key {
        config.insert("typesafe_api_key".into(), "fixture-not-a-secret".into());
    }
    let ctx = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "SemanticEvaluation".into(),
        entity_id: "e-1".into(),
        trigger_action: "Evaluate".into(),
        wasm_module: Some("evaluate_semantics".into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"Running","fields":{"parent_id":"p-1","world_id":"w-1","repair_log_file_id":"r-1","required_node_ids":"[]","round_count":"0","challenge_packet_json":packet_fixture(mode).0,"challenge_packet_sha256":packet_fixture(mode).1}}),
        agent_id: None,
        session_id: None,
        integration_config: config,
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
            Arc::new(host),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    serde_json::to_value(r).unwrap()
}
#[tokio::test]
async fn records_frozen_input_and_never_scores_a_path() {
    let r = evaluate("shadow", true, response()).await;
    assert_eq!(r["callback_action"], "Record", "{r}");
    let record: Value =
        serde_json::from_str(r["callback_params"]["result_json"].as_str().unwrap()).unwrap();
    assert_eq!(record["request"]["state"]["path_id"], "p-1");
    assert_eq!(record["response"]["model"], "jev-1.13.0");
    assert_eq!(record["routing"], "existing_critic_unchanged");
    assert_eq!(record["input_sha256"].as_str().unwrap().len(), 64);
    assert!(record["forecast_probability"].is_null());
    assert!(record["request"]["state"].get("challenge_flags").is_none());
}
#[tokio::test]
async fn off_skips_even_without_a_provider_credential() {
    let r = evaluate("off", false, json!({})).await;
    assert_eq!(r["callback_action"], "Skip", "{r}");
}
#[tokio::test]
async fn missing_credential_is_an_error_not_a_clear_judgment() {
    let r = evaluate("shadow", false, response()).await;
    assert_eq!(r["callback_action"], "Fail", "{r}");
    assert_eq!(
        r["callback_params"]["error_message"],
        "missing_typesafe_api_key"
    );
}
#[tokio::test]
async fn malformed_provider_output_fails_closed() {
    let r = evaluate("shadow", true, json!({"model":"jev-1.13.0","answers":{}})).await;
    assert_eq!(r["callback_action"], "Fail", "{r}");
}
#[test]
fn all_evaluation_outcomes_are_terminal_and_cannot_change_forecasts() {
    let source = include_str!("../../../os-apps/paw-foresight/specs/semantic_evaluation.ioa.toml");
    for (action, payload, status) in [
        ("Record", r#"{"result_json":"{}"}"#, "Recorded"),
        ("Skip", "{}", "Skipped"),
        ("Fail", r#"{"error_message":"test"}"#, "Failed"),
        ("Timeout", "{}", "Failed"),
    ] {
        let table = TransitionTable::from_ioa_source(source);
        let mut actor = EntityActorHandler::new("SemanticEvaluation", "e-1", Arc::new(table));
        actor.init().unwrap();
        actor.handle_message("Evaluate",r#"{"world_id":"w-1","endpoint_id":"ep-1","repair_log_file_id":"f-1","required_node_ids":"[]","round_count":"0"}"#).unwrap();
        actor.handle_message(action, payload).unwrap();
        assert_eq!(actor.current_status(), status);
        for (next, body) in [
            (
                "Evaluate",
                r#"{"world_id":"w-1","endpoint_id":"ep-1","repair_log_file_id":"f-1","required_node_ids":"[]","round_count":"0"}"#,
            ),
            ("Record", r#"{"result_json":"{}"}"#),
            ("Skip", "{}"),
            ("Fail", r#"{"error_message":"test"}"#),
            ("Timeout", "{}"),
        ] {
            assert!(
                actor.handle_message(next, body).is_err(),
                "terminal {status} accepted {next}"
            );
        }
    }
}

#[test]
fn evaluation_records_reject_generic_writes_and_untrusted_callbacks() {
    use temper_authz::{AuthzEngine, PrincipalKind, SecurityContext};
    use temper_server::request_context::AgentContext;
    let engine = AuthzEngine::new(include_str!(
        "../../../os-apps/paw-foresight/policies/foresight.cedar"
    ))
    .unwrap();
    let system = AgentContext::for_service("system").security_ctx.unwrap();
    let relay = AgentContext::for_service("wasm-runtime")
        .security_ctx
        .unwrap();
    let mut admin = SecurityContext::from_resolved_identity("dashboard", "human", None);
    admin.principal.kind = PrincipalKind::Admin;
    let attrs = std::collections::HashMap::from([("id".to_string(), json!("e-1"))]);
    assert!(
        engine
            .authorize(&system, "Evaluate", "SemanticEvaluation", &attrs)
            .is_allowed()
    );
    assert!(
        engine
            .authorize(&relay, "Record", "SemanticEvaluation", &attrs)
            .is_allowed()
    );
    for action in ["update", "delete"] {
        for caller in [&system, &relay, &admin] {
            assert!(
                !engine
                    .authorize(caller, action, "SemanticEvaluation", &attrs)
                    .is_allowed()
            );
        }
    }
    for action in ["Record", "Skip", "Fail", "Evaluate"] {
        assert!(
            !engine
                .authorize(&admin, action, "SemanticEvaluation", &attrs)
                .is_allowed()
        );
    }
}
#[test]
fn path_still_starts_the_original_critic_and_spawns_shadow_separately() {
    let spec: toml::Value = toml::from_str(include_str!(
        "../../../os-apps/paw-foresight/specs/path.ioa.toml"
    ))
    .unwrap();
    let a = spec["action"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"].as_str() == Some("LaunchChallenge"))
        .unwrap();
    assert_eq!(a["effect"].as_array().unwrap().len(), 2);
    assert_eq!(a["effect"][0]["name"].as_str(), Some("spawn_adversaries"));
    assert_eq!(
        a["effect"][1]["entity_type"].as_str(),
        Some("SemanticEvaluation")
    );
    TransitionTable::from_ioa_source(include_str!(
        "../../../os-apps/paw-foresight/specs/world.ioa.toml"
    ));
}
