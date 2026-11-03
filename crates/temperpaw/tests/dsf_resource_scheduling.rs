//! Resource-owned observation keeps running without another scheduling entity.
use serde_json::json;
use std::path::PathBuf;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::SpecRegistry,
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};

fn specs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/dsf-twin/specs")
}

#[test]
fn every_resource_owns_a_recurring_observation_timer() {
    for file in [
        "railway_service_instance",
        "vercel_project",
        "supabase_project",
        "cloudflare_r2_bucket",
        "datadog_monitor",
        "media_pipeline",
    ] {
        let source = std::fs::read_to_string(specs().join(format!("{file}.ioa.toml"))).unwrap();
        let ioa = temper_spec::automaton::parse_automaton(&source).unwrap();
        assert!(
            ioa.state_timeouts.iter().any(|timer| {
                timer.state == "Active"
                    && timer.on_timeout == "RefreshObservations"
                    && timer.after_seconds == 300
            }),
            "{file} has no recurring observation"
        );
        assert!(
            !ioa.state_timeouts
                .iter()
                .any(|timer| timer.state == "Retired")
        );
    }
}

#[tokio::test]
async fn failed_observation_rearms_and_retirement_stops_future_observation() {
    let xml = std::fs::read_to_string(specs().join("model.csdl.xml")).unwrap();
    let ioa = std::fs::read_to_string(specs().join("railway_service_instance.ioa.toml"))
        .unwrap()
        .replace("after_seconds = 300", "after_seconds = 1");
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("DsfRailwayServiceInstance", ioa.as_str())],
    );
    let server =
        ServerState::from_registry(ActorSystem::new("resource-observation-timers"), registry);
    let tenant = TenantId::from("default".to_owned());
    let agent = AgentContext::for_service("resource-timer-test");
    server
        .get_or_create_tenant_entity(&tenant, "DsfRailwayServiceInstance", "resource", json!({}))
        .await
        .unwrap();
    let dispatch = |action, params| {
        server.dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "DsfRailwayServiceInstance",
            entity_id: "resource",
            action,
            params,
            agent_ctx: &agent,
            await_integration: false,
            await_reactions: true,
        })
    };
    assert!(
        dispatch(
            "Register",
            json!({"project_id":"project", "service_id":"service",
        "environment_id":"environment", "config_ref":"config", "config_sha256":"digest"})
        )
        .await
        .unwrap()
        .success
    );
    for sequence in [1, 2] {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let row = server
            .get_tenant_entity_state(&tenant, "DsfRailwayServiceInstance", "resource")
            .await
            .unwrap();
        assert_eq!(row.state.status, "Refreshing");
        assert_eq!(row.state.counters.get("refresh_sequence"), Some(&sequence));
        assert!(
            dispatch(
                "CollectionFailed",
                json!({"expected_refresh_sequence":sequence,
            "error_message":"provider temporarily inaccessible"})
            )
            .await
            .unwrap()
            .success
        );
    }
    assert!(
        dispatch("Retire", json!({"effort_id":"effort"}))
            .await
            .unwrap()
            .success
    );
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let row = server
        .get_tenant_entity_state(&tenant, "DsfRailwayServiceInstance", "resource")
        .await
        .unwrap();
    assert_eq!(row.state.status, "Retired");
    assert_eq!(row.state.counters.get("refresh_sequence"), Some(&2));
}
