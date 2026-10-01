use serde_json::json;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::{EntityVerificationResult, SpecRegistry, VerificationStatus},
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};
#[tokio::test(flavor = "multi_thread")]
async fn denied_host_http_preserves_running_world_and_blocks_repeat_check() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-foresight");
    let src = std::fs::read_to_string(root.join("specs/world.ioa.toml"))
        .unwrap()
        .replacen("initial = \"Created\"", "initial = \"Seeding\"", 1)
        .replace("{secret:temper_api_url}", "http://127.0.0.1:3467");
    let src = src.replace(
        "name = \"research_session_id\"\ntype = \"string\"\ninitial = \"\"",
        "name = \"research_session_id\"\ntype = \"string\"\ninitial = \"child\"",
    );
    let src = if std::env::var_os("FORESIGHT_MONITOR_WITHOUT_RECOVERY").is_some() {
        src.replace("on_failure = \"ResearchMonitorInvocationFailed\"\n", "")
    } else {
        src
    };
    let xml = std::fs::read_to_string(root.join("specs/model.csdl.xml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("World", &src)],
    );
    let tenant = TenantId::default();
    registry.set_verification_status(
        &tenant,
        "World",
        VerificationStatus::Completed(EntityVerificationResult {
            all_passed: true,
            levels: vec![],
            verified_at: "2026-09-30".into(),
        }),
    );
    let state = ServerState::from_registry(ActorSystem::new("monitor-denial"), registry);
    state.rebuild_reaction_dispatcher();
    state
        .authz
        .reload_tenant_policies(
            "default",
            &std::fs::read_to_string(root.join("policies/foresight.cedar")).unwrap(),
        )
        .unwrap();
    let hash =
        state
            .wasm_engine
            .compile_and_cache(
                &std::fs::read(root.join(
                    "wasm/seed_session/target/wasm32-unknown-unknown/release/seed_session.wasm",
                ))
                .unwrap(),
            )
            .unwrap();
    state
        .wasm_module_registry
        .write()
        .unwrap()
        .register_builtin("seed_session", &hash);
    state
        .get_or_create_tenant_entity(&tenant, "World", "w", json!({}))
        .await
        .unwrap();
    let system = AgentContext::for_service("system");
    let out = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "World",
            entity_id: "w",
            action: "CheckResearchSession",
            params: json!({}),
            agent_ctx: &system,
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(out.success, "{out:?}");
    let row = state
        .get_tenant_entity_state(&tenant, "World", "w")
        .await
        .unwrap();
    assert_eq!(row.state.status, "Seeding");
    let v = serde_json::to_value(&row.state).unwrap();
    assert_eq!(v["booleans"]["research_monitor_unavailable"], true, "{v}");
    assert!(
        v["fields"]["error_message"]
            .as_str()
            .unwrap()
            .contains("authorization denied"),
        "{v}"
    );
    let again = state
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "World",
            entity_id: "w",
            action: "CheckResearchSession",
            params: json!({}),
            agent_ctx: &system,
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(!again.success);
}
