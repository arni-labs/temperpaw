//! Cancelling native reasoning propagates to its child session through the dispatcher.
use serde_json::json;
use std::path::PathBuf;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::{EntityVerificationResult, SpecRegistry, VerificationStatus},
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};

fn runtime(initial: &str, child: &str) -> ServerState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-foresight");
    let source = std::fs::read_to_string(root.join("specs/semantic_run.ioa.toml"))
        .unwrap()
        .replacen(
            "initial = \"Created\"",
            &format!("initial = \"{initial}\""),
            1,
        );
    let source = source.replace(
        "name = \"reasoning_session_id\"\ntype = \"string\"\ninitial = \"\"",
        &format!("name = \"reasoning_session_id\"\ntype = \"string\"\ninitial = \"{child}\""),
    );
    let session = r#"[automaton]
name = "Session"
states = ["Created", "Cancelled"]
initial = "Created"
[[action]]
name = "Cancel"
kind = "input"
from = ["Created"]
to = "Cancelled"
params = []
"#;
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<edmx:Edmx Version="4.0" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx"><edmx:DataServices><Schema Namespace="Test" xmlns="http://docs.oasis-open.org/odata/ns/edm">
<EntityType Name="SemanticRun"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String"/><Property Name="Status" Type="Edm.String"/><Property Name="reasoning_session_id" Type="Edm.String"/></EntityType>
<EntityType Name="Session"><Key><PropertyRef Name="Id"/></Key><Property Name="Id" Type="Edm.String"/><Property Name="Status" Type="Edm.String"/></EntityType>
<EntityContainer Name="Container"><EntitySet Name="SemanticRuns" EntityType="Test.SemanticRun"/><EntitySet Name="Sessions" EntityType="Test.Session"/></EntityContainer>
</Schema></edmx:DataServices></edmx:Edmx>"#;
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(xml).unwrap(),
        xml.into(),
        &[("SemanticRun", &source), ("Session", session)],
    );
    for name in ["SemanticRun", "Session"] {
        registry.set_verification_status(
            &TenantId::default(),
            name,
            VerificationStatus::Completed(EntityVerificationResult {
                all_passed: true,
                levels: vec![],
                verified_at: "2026-09-19T00:00:00Z".into(),
            }),
        );
    }
    let state = ServerState::from_registry(ActorSystem::new("semantic-cancel"), registry);
    state.rebuild_reaction_dispatcher();
    state.authz.reload_tenant_policies("default", "permit(principal, action, resource) when { principal has agent_type && principal.agent_type == \"system\" };").unwrap();
    state
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_reaches_existing_reasoning_child() {
    let state = runtime("Reasoning", "child-1");
    let tenant = TenantId::default();
    let system = AgentContext::for_service("system");
    state
        .get_or_create_tenant_entity(&tenant, "Session", "child-1", json!({}))
        .await
        .unwrap();
    state
        .get_or_create_tenant_entity(&tenant, "SemanticRun", "parent-1", json!({}))
        .await
        .unwrap();
    let result = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "SemanticRun",
            entity_id: "parent-1",
            action: "Cancel",
            params: json!({}),
            agent_ctx: &system,
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(result.success, "{result:?}");
    assert_eq!(
        state
            .get_tenant_entity_state(&tenant, "SemanticRun", "parent-1")
            .await
            .unwrap()
            .state
            .status,
        "Cancelled"
    );
    for _ in 0..40 {
        if state
            .get_tenant_entity_state(&tenant, "Session", "child-1")
            .await
            .unwrap()
            .state
            .status
            == "Cancelled"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(
        state
            .get_tenant_entity_state(&tenant, "Session", "child-1")
            .await
            .unwrap()
            .state
            .status,
        "Cancelled"
    );
    assert_eq!(state.list_entity_ids(&tenant, "Session"), vec!["child-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_without_child_creates_nothing_and_blocks_late_spawn() {
    let state = runtime("ReasoningSetup", "");
    let tenant = TenantId::default();
    let system = AgentContext::for_service("system");
    state
        .get_or_create_tenant_entity(&tenant, "SemanticRun", "parent-empty", json!({}))
        .await
        .unwrap();
    let result = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "SemanticRun",
            entity_id: "parent-empty",
            action: "Cancel",
            params: json!({}),
            agent_ctx: &system,
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(result.success, "{result:?}");
    assert_eq!(
        state
            .get_tenant_entity_state(&tenant, "SemanticRun", "parent-empty")
            .await
            .unwrap()
            .state
            .status,
        "Cancelled"
    );
    assert!(state.list_entity_ids(&tenant, "Session").is_empty());
    let late = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "SemanticRun",
            entity_id: "parent-empty",
            action: "SpawnReasoning",
            params: json!({}),
            agent_ctx: &system,
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(!late.success, "cancelled parent must reject delayed spawn");
    assert!(state.list_entity_ids(&tenant, "Session").is_empty());
}
