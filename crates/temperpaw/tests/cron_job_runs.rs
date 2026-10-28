//! CronJob runs under the real CronJob spec, an in-process Temper server and the
//! packaged cron_compute_next module; no Session runs and no provider calls.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use temper_authz::{AuthzDecision, AuthzEngine, Principal, PrincipalKind, SecurityContext};
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::SpecRegistry,
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits,
};

const JOB: &str = "cj-weekly-digest";
const MESSAGE: &str = "Write this week's digest from the notes folder.";

fn app() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-agent")
}

fn tenant() -> TenantId {
    TenantId::from("default".to_owned())
}

/// A server holding only the CronJob spec, with one job created the way an
/// OData create makes it: its fields come from the create body, not Configure.
async fn server_with_job(fields: Value) -> ServerState {
    let xml = std::fs::read_to_string(app().join("specs/model.csdl.xml")).unwrap();
    let ioa = std::fs::read_to_string(app().join("specs/cron_job.ioa.toml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("CronJob", ioa.as_str())],
    );
    let server = ServerState::from_registry(ActorSystem::new("cron-job-runs"), registry);
    server
        .get_or_create_tenant_entity(&tenant(), "CronJob", JOB, fields)
        .await
        .unwrap();
    act(&server, "Activate", json!({})).await;
    // Far ahead, so no scheduled Trigger fires while the test drives the runs.
    act(
        &server,
        "ActivateComplete",
        json!({"next_run_at": "2099-01-01T00:00:00Z"}),
    )
    .await;
    server
}

async fn act(server: &ServerState, action: &str, params: Value) {
    let tenant = tenant();
    let agent = AgentContext::for_service("cron-job-runs-test");
    let response = server
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "CronJob",
            entity_id: JOB,
            action,
            params,
            agent_ctx: &agent,
            await_integration: false,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(response.success, "{action} refused: {:?}", response.error);
}

async fn job(server: &ServerState) -> Value {
    let row = server
        .get_tenant_entity_state(&tenant(), "CronJob", JOB)
        .await
        .unwrap();
    serde_json::to_value(&row.state).unwrap()
}

/// One run: Trigger, the packaged module's answer to it, and that answer
/// applied as TriggerComplete. Returns what TriggerComplete carried.
async fn run_once(server: &ServerState, run: u32) -> Value {
    act(
        server,
        "Trigger",
        json!({"last_run_at": format!("2026-10-24T00:{run:02}:00Z")}),
    )
    .await;
    let context = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "CronJob".into(),
        entity_id: JOB.into(),
        trigger_action: "Trigger".into(),
        wasm_module: Some("cron_compute_next".into()),
        trigger_params: json!({}),
        entity_state: job(server).await,
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([("mode".into(), "trigger".into())]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let bytes = std::fs::read(app().join("wasm/cron_compute_next/cron_compute_next.wasm"))
        .unwrap_or_else(|error| panic!("Build paw-agent WASMs before this proof: {error}"));
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let result = engine
        .invoke(
            &hash,
            &context,
            Arc::new(SimWasmHost::new()),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert!(result.success, "{result:?}");
    assert_eq!(result.callback_action, "TriggerComplete");
    act(server, "TriggerComplete", result.callback_params.clone()).await;
    result.callback_params
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_without_a_template_gives_every_run_its_own_message() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "system_prompt": "You write a short digest.",
        "user_message": MESSAGE,
        "model": "gpt-5.5",
        "provider": "openai_codex",
    }))
    .await;
    for run in 1..=3 {
        let completed = run_once(&server, run).await;
        // TriggerComplete's params win over copied fields in the Session it
        // spawns, so this is the message that run's Session gets.
        assert_eq!(completed["user_message"], MESSAGE, "run {run}");
        assert_eq!(
            job(&server).await["fields"]["user_message"],
            MESSAGE,
            "after run {run}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_template_still_writes_each_runs_message() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "user_message_template": "Write this week's digest; the last one said: {{last_result}}.",
        "user_message": "An older message.",
        "last_result": "three notes",
        "model": "gpt-5.5",
        "provider": "openai_codex",
    }))
    .await;
    let completed = run_once(&server, 1).await;
    assert_eq!(
        completed["user_message"],
        "Write this week's digest; the last one said: three notes."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn record_last_session_sets_the_session_a_job_names() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "user_message": MESSAGE,
        "last_session_id": "019e0000-0000-7000-8000-00000000000a",
    }))
    .await;
    act(&server, "Pause", json!({})).await;
    act(
        &server,
        "RecordLastSession",
        json!({"last_session_id": "019e0000-0000-7000-8000-00000000000b"}),
    )
    .await;
    let state = job(&server).await;
    assert_eq!(
        state["fields"]["last_session_id"],
        "019e0000-0000-7000-8000-00000000000b"
    );
    assert_eq!(state["status"], "Paused");
}

#[test]
fn only_the_verified_operator_may_record_a_jobs_last_session() {
    let policy = std::fs::read_to_string(app().join("policies/cron.cedar")).unwrap();
    let engine = AuthzEngine::new(&policy).unwrap();
    let job = HashMap::from([
        ("id".to_string(), json!(JOB)),
        ("status".to_string(), json!("Paused")),
    ]);
    let allowed = |context: &SecurityContext, action: &str| {
        matches!(
            engine.authorize(context, action, "CronJob", &job),
            AuthzDecision::Allow { .. }
        )
    };
    let admin = SecurityContext {
        principal: Principal {
            id: "admin".to_string(),
            kind: PrincipalKind::Admin,
            role: None,
            acting_for: None,
            agent_type: None,
            attributes: HashMap::new(),
        },
        context_attrs: HashMap::new(),
        correlation_id: "cron-job-runs-test".to_string(),
        cause: None,
    };
    let operator = SecurityContext::from_resolved_identity("operator", "operator", None);
    let refused = [
        ("admin", admin.clone()),
        (
            "person",
            SecurityContext::from_resolved_identity("person", "human", None),
        ),
        (
            "supervisor",
            SecurityContext::from_resolved_identity("lead", "supervisor", None),
        ),
        (
            "system agent",
            SecurityContext::from_resolved_identity("scheduler", "system", None),
        ),
        (
            "agent",
            SecurityContext::from_resolved_identity("helper", "agent", None),
        ),
        (
            "unverified operator claim",
            SecurityContext::anonymous().with_agent_context(
                Some("claimed-operator"),
                None,
                Some("operator"),
            ),
        ),
    ];
    for action in ["RecordLastSession", "CronJob.RecordLastSession"] {
        assert!(allowed(&operator, action), "operator refused {action}");
        for (who, context) in &refused {
            assert!(!allowed(context, action), "{who} allowed {action}");
        }
    }
    // The repair move is carved out alone: Admin keeps every other move.
    assert!(allowed(&admin, "Pause"));
}
