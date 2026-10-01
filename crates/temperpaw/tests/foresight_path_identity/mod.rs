//! Exercise background corridor callbacks across the real declared system stages.
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use temper_authz::AuthenticatedRequestContext;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::{EntityVerificationResult, SpecRegistry, VerificationStatus},
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};
use tower::ServiceExt;

async fn wait_for_action_count(
    state: &ServerState,
    entity: &str,
    id: &str,
    action: &str,
    count: usize,
) {
    let tenant = TenantId::default();
    for _ in 0..200 {
        let row = state
            .get_tenant_entity_state(&tenant, entity, id)
            .await
            .unwrap();
        if row
            .state
            .events
            .iter()
            .filter(|event| event.action == action)
            .count()
            >= count
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let row = state
        .get_tenant_entity_state(&tenant, entity, id)
        .await
        .unwrap();
    panic!(
        "{entity}.{action} count {count} was not reached: {:?}",
        row.state
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn path_callbacks_reenter_system_before_deterministic_http_writes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = root.join("os-apps/paw-foresight");
    let xml = std::fs::read_to_string(app.join("specs/model.csdl.xml")).unwrap();
    let mut path = std::fs::read_to_string(app.join("specs/path.ioa.toml"))
        .unwrap()
        .replace("{secret:temper_api_url}", "http://127.0.0.1:3000");
    let production = temper_jit::TransitionTable::from_ioa_source(&path);
    assert!(
        production
            .evaluate("Solving", 0, "ResumeRepair")
            .unwrap()
            .success
    );
    // End this bounded test at the adversary launch boundary. Preserve source
    // order: the runtime parser consumes ordered IOA sections. The real Path
    // transitions and aggregate WASM still run; provider sessions run live.
    let stage_start = path
        .find("[[action]]\nname = \"StartChallenge\"\n")
        .expect("production Path must declare StartChallenge");
    let after_header = stage_start + "[[action]]".len();
    let stage_end = path[after_header..]
        .find("\n[[action]]")
        .map_or(path.len(), |offset| after_header + offset);
    let effect_start = stage_start
        + path[stage_start..stage_end]
            .find("\n[[action.triggers]]")
            .expect("StartChallenge must have its provider integration");
    path.replace_range(effect_start..stage_end, "\n");
    // A terminal receiver records the actual authenticated HTTP relay. Claim
    // decision policy remains production policy; further costing is out of scope.
    let claim = r#"
[automaton]
name = "Claim"
states = ["Bridging"]
initial = "Bridging"
[[state]]
name = "path_id"
type = "string"
initial = ""
[[state]]
name = "route_cost"
type = "string"
initial = ""
[[action]]
name = "RouteSettled"
kind = "input"
from = ["Bridging"]
params = ["path_id", "route_cost"]
"#;
    // Costing selects its existing deep-search budget from the authoritative
    // World. This fixture preserves that policy without launching more workers.
    let world = r#"
[automaton]
name = "World"
states = ["Active"]
initial = "Active"
allow_indefinite_states = ["Active"]
[[state]]
name = "exploration_phase"
type = "string"
initial = "deepening"
"#;
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("Path", path.as_str()), ("Claim", claim), ("World", world)],
    );
    for entity in ["Path", "Claim", "World"] {
        registry.set_verification_status(
            &TenantId::default(),
            entity,
            VerificationStatus::Completed(EntityVerificationResult {
                all_passed: true,
                levels: vec![],
                verified_at: "2026-09-16T00:00:00Z".into(),
            }),
        );
    }
    let state = ServerState::from_registry(ActorSystem::new("foresight-path-identity"), registry);
    state.rebuild_reaction_dispatcher();
    state
        .authz
        .reload_tenant_policies(
            "default",
            &std::fs::read_to_string(app.join("policies/foresight.cedar")).unwrap(),
        )
        .unwrap();
    let bytes = super::module_bytes("aggregate_costs");
    let hash = state.wasm_engine.compile_and_cache(&bytes).unwrap();
    state
        .wasm_module_registry
        .write()
        .unwrap()
        .register_builtin("aggregate_costs", &hash);
    let tenant = TenantId::default();
    state
        .get_or_create_tenant_entity(&tenant, "World", "world-1", json!({}))
        .await
        .unwrap();
    state
        .get_or_create_tenant_entity(&tenant, "Claim", "claim-1", json!({}))
        .await
        .unwrap();
    state
        .get_or_create_tenant_entity(
            &tenant,
            "Path",
            "path-1",
            json!({"claim_id":"claim-1", "world_id":"world-1"}),
        )
        .await
        .unwrap();
    let relay = AgentContext::for_service("wasm-runtime");
    for action in [
        "EvaluateRepair",
        "EvaluateChallenge",
        "RelayScoredRoute",
        "RelayPrunedRoute",
        "StartChallenge",
        "ResumeRepair",
        "Score",
        "Prune",
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/tdata/Paths('path-1')/TemperPaw.{action}"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        request
            .extensions_mut()
            .insert(AuthenticatedRequestContext::new(
                tenant.clone(),
                relay.security_ctx.clone().unwrap(),
            ));
        let response = temper_server::build_router(state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "session relay must not invoke Path.{action} directly"
        );
    }
    // Use background integration mode: generated callbacks reset to the WASM
    // service, then their declared entity triggers must restore system identity.
    let repair = state.dispatch(DispatchCommand {
        tenant: &tenant, entity_type: "Path", entity_id: "path-1", action: "RepairComplete",
        params: json!({"repair_log_file_id":"fixture-repair","required_node_ids":[],"cost_flags":[]}),
        agent_ctx: &relay, await_integration: false, await_reactions: true,
    }).await.unwrap();
    assert!(repair.success, "{repair:?}");
    wait_for_action_count(&state, "Path", "path-1", "StartChallenge", 1).await;
    let challenge = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "Path",
            entity_id: "path-1",
            action: "ChallengeComplete",
            params: json!({"challenge_log_file_id":"fixture-challenge","challenge_flags":[]}),
            agent_ctx: &relay,
            await_integration: false,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(challenge.success, "{challenge:?}");
    wait_for_action_count(&state, "Claim", "claim-1", "RouteSettled", 1).await;
    let route = state
        .get_tenant_entity_state(&tenant, "Path", "path-1")
        .await
        .unwrap();
    for action in [
        "EvaluateRepair",
        "RequestChallenge",
        "StartChallenge",
        "EvaluateChallenge",
        "Score",
        "RelayScoredRoute",
    ] {
        assert_eq!(
            route
                .state
                .events
                .iter()
                .filter(|event| event.action == action)
                .count(),
            1,
            "the declared Path stage {action} must execute exactly once"
        );
    }
    let claim = state
        .get_tenant_entity_state(&tenant, "Claim", "claim-1")
        .await
        .unwrap();
    assert_eq!(claim.state.fields["path_id"], "path-1");
    let route_cost = claim.state.fields["route_cost"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert_eq!(route_cost, 0.0);
    assert_eq!(
        claim
            .state
            .events
            .iter()
            .filter(|event| event.action == "RouteSettled")
            .count(),
        1
    );

    // The first route costs zero. A high contradiction costs 25 * 2 = 50,
    // exceeding its sibling's pruning ceiling (0 * 2 + 20). The same real
    // WASM must prune this route, report that cost, and retain its decision.
    state
        .get_or_create_tenant_entity(
            &tenant,
            "Path",
            "path-pruned",
            json!({"claim_id":"claim-1", "world_id":"world-1"}),
        )
        .await
        .unwrap();
    let repair = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "Path",
            entity_id: "path-pruned",
            action: "RepairComplete",
            params: json!({
                "repair_log_file_id":"fixture-pruned-repair",
                "required_node_ids":[],
                "cost_flags":[{"kind":"contradiction","severity":"high","note":"fixture"}]
            }),
            agent_ctx: &relay,
            await_integration: false,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(repair.success, "{repair:?}");
    wait_for_action_count(&state, "Claim", "claim-1", "RouteSettled", 2).await;
    let pruned = state
        .get_tenant_entity_state(&tenant, "Path", "path-pruned")
        .await
        .unwrap();
    assert_eq!(pruned.state.status, "Pruned");
    for action in ["EvaluateRepair", "Prune", "RelayPrunedRoute"] {
        assert_eq!(
            pruned
                .state
                .events
                .iter()
                .filter(|event| event.action == action)
                .count(),
            1,
            "the pruned route must execute {action} exactly once"
        );
    }
    assert!(
        !pruned
            .state
            .events
            .iter()
            .any(|event| event.action == "StartChallenge")
    );
    let claim = state
        .get_tenant_entity_state(&tenant, "Claim", "claim-1")
        .await
        .unwrap();
    assert_eq!(claim.state.fields["path_id"], "path-pruned");
    let pruned_cost = claim.state.fields["route_cost"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert_eq!(pruned_cost, 50.0);
    assert_eq!(
        claim
            .state
            .events
            .iter()
            .filter(|event| event.action == "RouteSettled")
            .count(),
        2
    );
}
