use serde_json::json;
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    ServerState, SpecRegistry,
    entity_actor::{EntityState, process_action},
    request_context::AgentContext,
    state::DispatchCommand,
};
const IOA: &str = include_str!("../../../os-apps/paw-foresight/specs/semantic_run.ioa.toml");
const CSDL: &str = include_str!("../../../os-apps/paw-foresight/specs/model.csdl.xml");
#[test]
fn pending_and_progress_each_schedule_exactly_one_poll_and_keep_run_limits() {
    let _clock = temper_runtime::scheduler::install_deterministic_context(519);
    let table = temper_jit::TransitionTable::from_ioa_source(IOA);
    let mut state: EntityState = serde_json::from_value(json!({"entity_type":"SemanticRun","entity_id":"progress","status":"Reasoning","item_count":0,"fields":{"started_at_ms":"123","program_json":"checkpoint"},"counters":{"transition_count":10,"check_count":2}})).unwrap();
    for (action, params) in [
        ("ReasoningPending", json!({})),
        (
            "ReasoningProgress",
            json!({"reasoning_progress_session_id":"child","reasoning_progress_token":"8"}),
        ),
    ] {
        let before = state.counters["transition_count"];
        let result = process_action(&mut state, &table, action, &params);
        assert!(result.success, "{result:?}");
        assert_eq!(state.status, "Reasoning");
        assert_eq!(state.counters["transition_count"], before + 1);
        assert_eq!(state.fields["started_at_ms"], "123");
        assert_eq!(state.fields["program_json"], "checkpoint");
        assert_eq!(result.scheduled_actions.len(), 1);
        let schedule = serde_json::to_value(&result.scheduled_actions[0]).unwrap();
        assert_eq!(schedule["action"], "CheckReasoning");
        assert_eq!(schedule["delay_seconds"], 60);
    }
    assert_eq!(state.fields["reasoning_progress_session_id"], "child");
    assert_eq!(state.fields["reasoning_progress_token"], "8");
}
async fn dispatch(state: &ServerState, tenant: &TenantId, action: &str, params: serde_json::Value) {
    let agent = AgentContext::for_service("timeout-scheduler");
    let out = state
        .dispatch(DispatchCommand {
            tenant,
            entity_type: "SemanticRun",
            entity_id: "progress",
            action,
            params,
            agent_ctx: &agent,
            await_integration: false,
            await_reactions: false,
        })
        .await
        .unwrap();
    assert!(out.success, "{out:?}");
}
async fn timer_fixture(reset_on_progress: bool) -> (ServerState, TenantId) {
    // Only timer scale and initial state differ from the actual application spec.
    let mut source = IOA
        .replacen("initial = \"Created\"", "initial = \"Reasoning\"", 1)
        .replace(
            "state = \"Reasoning\"\nafter_seconds = 900",
            "state = \"Reasoning\"\nafter_seconds = 2",
        );
    if !reset_on_progress {
        source = source.replace("reset_on = [\"ReasoningProgress\"]", "");
    }
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(CSDL).unwrap(),
        CSDL.to_string(),
        &[("SemanticRun", source.as_str())],
    );
    let state = ServerState::from_registry(
        ActorSystem::new(if reset_on_progress {
            "progress-idle"
        } else {
            "progress-negative"
        }),
        registry,
    );
    let tenant = TenantId::from("default".to_owned());
    state
        .get_or_create_tenant_entity(&tenant, "SemanticRun", "progress", json!({}))
        .await
        .unwrap();
    dispatch(&state, &tenant, "ReasoningPending", json!({})).await;
    (state, tenant)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_idle_timer_resets_only_on_progress_and_old_absolute_timer_fails() {
    let (state, tenant) = timer_fixture(true).await;
    let (old, oldtenant) = timer_fixture(false).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    for (s, t) in [(&state, &tenant), (&old, &oldtenant)] {
        dispatch(
            s,
            t,
            "ReasoningProgress",
            json!({"reasoning_progress_session_id":"child","reasoning_progress_token":"8"}),
        )
        .await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(
        state
            .get_tenant_entity_state(&tenant, "SemanticRun", "progress")
            .await
            .unwrap()
            .state
            .status,
        "Reasoning"
    );
    assert_eq!(
        old.get_tenant_entity_state(&oldtenant, "SemanticRun", "progress")
            .await
            .unwrap()
            .state
            .status,
        "Failed",
        "removing reset_on must reproduce the absolute timeout"
    );
    dispatch(&state, &tenant, "ReasoningPending", json!({})).await;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(
        state
            .get_tenant_entity_state(&tenant, "SemanticRun", "progress")
            .await
            .unwrap()
            .state
            .status,
        "Failed",
        "unchanged polls must not postpone the idle deadline"
    );
}

#[test]
fn progress_callback_is_runtime_only() {
    use temper_authz::{AuthzEngine, SecurityContext};
    let engine = AuthzEngine::new(include_str!(
        "../../../os-apps/paw-foresight/policies/foresight.cedar"
    ))
    .unwrap();
    let attrs = std::collections::HashMap::new();
    let runtime = SecurityContext::from_resolved_identity("service:wasm-runtime", "system", None);
    for action in ["ReasoningProgress", "ResumeReasoned"] {
        assert!(
            engine
                .authorize(&runtime, action, "SemanticRun", &attrs)
                .is_allowed()
        );
        for (id, kind) in [
            ("operator", "operator"),
            ("ordinary", "agent"),
            ("other-system", "system"),
        ] {
            let principal = SecurityContext::from_resolved_identity(id, kind, None);
            assert!(
                !engine
                    .authorize(&principal, action, "SemanticRun", &attrs)
                    .is_allowed()
            );
        }
    }
}
