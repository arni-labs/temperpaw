//! The real barrier WASM must enter through the declared deterministic principal.
use axum::{body::Body, http::Request};
use serde_json::json;
use std::path::PathBuf;
use temper_authz::AuthenticatedRequestContext;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::{EntityVerificationResult, SpecRegistry, VerificationStatus},
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};
use tower::ServiceExt;

fn module_bytes(module: &str) -> Vec<u8> {
    let path = if let Some(dir) = std::env::var_os("FORESIGHT_WASM_DIR") {
        PathBuf::from(dir).join(format!("{module}.wasm"))
    } else {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new("bash")
            .current_dir(&root)
            .args(["-c", "set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm \"$1\" wasm32-unknown-unknown --locked", "foresight-gate-test"])
            .arg(root.join(format!("os-apps/paw-foresight/wasm/{module}")))
            .output().expect("run canonical WASM builder");
        assert!(
            output.status.success(),
            "build {module}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    };
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[tokio::test(flavor = "multi_thread")]
async fn bundle_written_runs_barrier_as_system_without_granting_the_session_gate_access() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = root.join("os-apps/paw-foresight");
    let xml = std::fs::read_to_string(app.join("specs/model.csdl.xml")).unwrap();
    let before = std::env::var_os("FORESIGHT_GATE_BEFORE_DIR").map(PathBuf::from);
    let endpoint_path = before.as_ref().map_or_else(
        || app.join("specs/endpoint.ioa.toml"),
        |d| d.join("endpoint.ioa.toml"),
    );
    let endpoint = std::fs::read_to_string(endpoint_path)
        .unwrap()
        .replace("{secret:temper_api_url}", "http://127.0.0.1:3000");
    // The destination deliberately ends at GateDiversity. This test proves the
    // actual actor -> trigger -> WASM -> authenticated OData boundary; provider
    // work and the rest of the corridor are exercised by the live acceptance.
    let world = r#"
[automaton]
name = "World"
states = ["Active"]
initial = "Active"
[[state]]
name = "gate_rounds"
type = "string"
initial = "0"
[[action]]
name = "GateDiversity"
kind = "input"
from = ["Active"]
params = ["gate_rounds"]
"#;
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("Endpoint", endpoint.as_str()), ("World", world)],
    );
    for entity in ["World", "Endpoint"] {
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
    let state = ServerState::from_registry(ActorSystem::new("foresight-gate-identity"), registry);
    state.rebuild_reaction_dispatcher();
    let policy_path = before.as_ref().map_or_else(
        || app.join("policies/foresight.cedar"),
        |d| d.join("foresight.cedar"),
    );
    state
        .authz
        .reload_tenant_policies("default", &std::fs::read_to_string(policy_path).unwrap())
        .unwrap();
    let bytes = module_bytes("sample_endpoints");
    let hash = state.wasm_engine.compile_and_cache(&bytes).unwrap();
    state
        .wasm_module_registry
        .write()
        .unwrap()
        .register_builtin("sample_endpoints", &hash);
    let tenant = TenantId::default();
    state
        .get_or_create_tenant_entity(&tenant, "World", "world-1", json!({}))
        .await
        .unwrap();
    state
        .get_or_create_tenant_entity(
            &tenant,
            "Endpoint",
            "endpoint-1",
            json!({"world_id":"world-1"}),
        )
        .await
        .unwrap();
    let relay = AgentContext::for_service("wasm-runtime");
    for (entity, id, action, params) in [
        (
            "World",
            "world-1",
            "GateDiversity",
            json!({"gate_rounds":"99"}),
        ),
        ("Endpoint", "endpoint-1", "CheckDiversityBarrier", json!({})),
    ] {
        let set = if entity == "World" {
            "Worlds"
        } else {
            "Endpoints"
        };
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/tdata/{set}('{id}')/TemperPaw.{action}"))
            .header("content-type", "application/json")
            .body(Body::from(params.to_string()))
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
            axum::http::StatusCode::FORBIDDEN,
            "session relay must not invoke {entity}.{action} directly"
        );
    }
    let result = state.dispatch(DispatchCommand {tenant:&tenant,entity_type:"Endpoint",entity_id:"endpoint-1",
        action:"BundleWritten",params:json!({"bundle_file_id":"fixture-bundle","summary":"fixture","author_agent_id":"writer"}),
        agent_ctx:&relay,await_integration:true,await_reactions:true}).await.unwrap();
    assert!(result.success, "{result:?}");
    let world = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let world = state
                .get_tenant_entity_state(&tenant, "World", "world-1")
                .await
                .unwrap();
            if world
                .state
                .events
                .iter()
                .any(|event| event.action == "GateDiversity")
            {
                break world;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("declared barrier WASM must commit GateDiversity within the bounded wait");
    let endpoint = state
        .get_tenant_entity_state(&tenant, "Endpoint", "endpoint-1")
        .await
        .unwrap();
    assert!(
        endpoint
            .state
            .events
            .iter()
            .any(|event| event.action == "CheckDiversityBarrier"),
        "declared barrier action must execute: {:?}",
        endpoint.state.events
    );
    assert_eq!(
        world.state.fields["gate_rounds"], "1",
        "real WASM must advance the gate under its declared principal: {:?}",
        world.state
    );
    assert_eq!(
        world
            .state
            .events
            .iter()
            .filter(|event| event.action == "GateDiversity")
            .count(),
        1
    );
}

#[path = "foresight_path_identity/mod.rs"]
mod path_identity;

#[test]
fn deterministic_entry_actions_reject_direct_dashboard_and_session_calls() {
    use temper_authz::{AuthzEngine, PrincipalKind, SecurityContext};
    let policy = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/policies/foresight.cedar"),
    )
    .unwrap();
    let engine = AuthzEngine::new(&policy).unwrap();
    let system = AgentContext::for_service("system").security_ctx.unwrap();
    let relay = AgentContext::for_service("wasm-runtime")
        .security_ctx
        .unwrap();
    let mut admin = SecurityContext::from_resolved_identity("dashboard", "human", None);
    admin.principal.kind = PrincipalKind::Admin;
    for (entity, actions) in [
        ("Endpoint", vec!["CheckDiversityBarrier", "PrepareClaims"]),
        ("Claim", vec!["PrepareBridge"]),
        (
            "Path",
            vec![
                "EvaluateRepair",
                "EvaluateChallenge",
                "RelayScoredRoute",
                "RelayPrunedRoute",
                "StartChallenge",
                "LaunchChallenge",
            ],
        ),
    ] {
        for action in actions {
            let attrs = std::collections::HashMap::from([("id".to_string(), json!("fixture"))]);
            assert!(
                engine
                    .authorize(&system, action, entity, &attrs)
                    .is_allowed(),
                "declared system entry {entity}.{action}"
            );
            for caller in [&relay, &admin] {
                assert!(
                    !engine
                        .authorize(caller, action, entity, &attrs)
                        .is_allowed(),
                    "direct {} must not enter {entity}.{action}",
                    caller.principal.id
                );
            }
        }
    }
}

#[test]
fn corridor_entry_triggers_and_metadata_preserve_the_system_boundary() {
    use temper_spec::automaton::{TargetResolver, TriggerKind, parse_automaton};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-foresight/specs");
    let xml = std::fs::read_to_string(root.join("model.csdl.xml")).unwrap();
    let metadata = temper_spec::csdl::parse_csdl(&xml).unwrap();
    for (entity, file, entries) in [
        (
            "Endpoint",
            "endpoint",
            vec![
                ("BundleWritten", "CheckDiversityBarrier"),
                ("DecompositionComplete", "PrepareClaims"),
            ],
        ),
        ("Claim", "claim", vec![("SubmitForBridge", "PrepareBridge")]),
        (
            "Path",
            "path",
            vec![
                ("RepairComplete", "EvaluateRepair"),
                ("ChallengeComplete", "EvaluateChallenge"),
                ("Score", "RelayScoredRoute"),
                ("Prune", "RelayPrunedRoute"),
                ("RequestChallenge", "StartChallenge"),
                ("ChallengePrepared", "LaunchChallenge"),
                ("RevisionRequested", "ResumeRepair"),
            ],
        ),
    ] {
        let ioa = parse_automaton(
            &std::fs::read_to_string(root.join(format!("{file}.ioa.toml"))).unwrap(),
        )
        .unwrap();
        for (source, target) in entries {
            let action = ioa.actions.iter().find(|a| a.name == source).unwrap();
            assert!(
                !action.triggers.iter().any(|t| t.kind == TriggerKind::Wasm),
                "{entity}.{source} must enter deterministic work through the named reaction"
            );
            let triggers: Vec<_> = action
                .triggers
                .iter()
                .filter(|t| t.target_action.as_deref() == Some(target))
                .collect();
            assert_eq!(triggers.len(), 1, "{entity}.{source}");
            let trigger = triggers[0];
            assert_eq!(trigger.kind, TriggerKind::Entity);
            assert_eq!(trigger.principal.as_deref(), Some("system"));
            assert_eq!(trigger.target_entity.as_deref(), Some(entity));
            assert_eq!(trigger.resolve_target, Some(TargetResolver::SameId));
            assert!(
                ioa.actions
                    .iter()
                    .any(|a| a.name == target && a.params.is_empty())
            );
            let binding = format!("TemperPaw.Foresight.{entity}");
            let entries: Vec<_> = metadata
                .schemas
                .iter()
                .flat_map(|schema| &schema.actions)
                .filter(|a| a.name == target && a.binding_type() == Some(binding.as_str()))
                .collect();
            assert_eq!(
                entries.len(),
                1,
                "{entity}.{target} must have exactly one bound metadata action"
            );
            assert_eq!(
                entries[0].parameters.len(),
                1,
                "only the binding parameter is public"
            );
        }
    }
}

fn pruned_relay_preserves_decision(table: &temper_jit::TransitionTable) -> bool {
    let outgoing: Vec<_> = table
        .rules
        .iter()
        .filter(|rule| {
            rule.from_states.is_empty() || rule.from_states.iter().any(|state| state == "Pruned")
        })
        .collect();
    outgoing.len() == 1
        && outgoing[0].name == "RelayPrunedRoute"
        && outgoing[0].to_state.as_deref().unwrap_or("Pruned") == "Pruned"
        && outgoing[0].effects.iter().all(|effect| {
            !matches!(effect, temper_jit::table::Effect::SetState(state) if state != "Pruned")
        })
}

#[test]
fn pruned_route_can_only_report_without_reopening_the_decision() {
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-foresight/specs/path.ioa.toml"),
    )
    .unwrap();
    let mut table = temper_jit::TransitionTable::from_ioa_source(&source);
    assert!(pruned_relay_preserves_decision(&table));
    // Negative control: a relay that reopens repair must fail the same rule.
    table
        .rules
        .iter_mut()
        .find(|rule| rule.name == "RelayPrunedRoute")
        .unwrap()
        .to_state = Some("Solving".into());
    assert!(!pruned_relay_preserves_decision(&table));
}

#[path = "foresight_provider_profile/mod.rs"]
mod provider_profile;
