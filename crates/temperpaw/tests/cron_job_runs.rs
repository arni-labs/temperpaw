//! CronJob runs under the real CronJob and Session specs, an in-process Temper
//! server and the packaged cron_compute_next module. A run's Session is
//! spawned and configured, but no Session module runs and no provider calls.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use temper_authz::{AuthzDecision, AuthzEngine, Principal, PrincipalKind, SecurityContext};
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::SpecRegistry,
    request_context::AgentContext,
    secrets::{SecretsVault, resolve_secret_templates},
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

/// A server holding the CronJob and Session specs, with one job created the
/// way an OData create makes it: its fields come from the create body, not
/// Configure.
async fn server_with_job(fields: Value) -> ServerState {
    let xml = std::fs::read_to_string(app().join("specs/model.csdl.xml")).unwrap();
    let ioa = std::fs::read_to_string(app().join("specs/cron_job.ioa.toml")).unwrap();
    let session = std::fs::read_to_string(app().join("specs/session.ioa.toml")).unwrap();
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("CronJob", ioa.as_str()), ("Session", session.as_str())],
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

/// The fields of the Session the job's last run spawned, once its Configure
/// has applied. The server spawns it after TriggerComplete returns.
async fn spawned_session(server: &ServerState) -> Value {
    let session_id = job(server).await["fields"]["last_session_id"]
        .as_str()
        .expect("TriggerComplete names the Session it spawns")
        .to_string();
    for _ in 0..400 {
        if server.entity_exists(&tenant(), "Session", &session_id) {
            let row = server
                .get_tenant_entity_state(&tenant(), "Session", &session_id)
                .await
                .unwrap();
            let state = serde_json::to_value(&row.state).unwrap();
            // Configure schedules ProvisionWorkspace even for an empty message.
            if state["status"] != "Created" {
                return state["fields"].clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Session {session_id} never applied its Configure");
}

/// The Trigger move's module config as the server hands it over: the spec's
/// config with the tenant's `{secret:...}` templates resolved, and any the
/// tenant does not hold left as written.
fn trigger_config(tenant_secrets: &[(&str, &str)]) -> BTreeMap<String, String> {
    let ioa = std::fs::read_to_string(app().join("specs/cron_job.ioa.toml")).unwrap();
    let spec = temper_spec::automaton::parse_automaton(&ioa).unwrap();
    let integration = spec
        .integrations
        .iter()
        .find(|integration| integration.name == "cron_compute_next")
        .expect("the Trigger move runs cron_compute_next");
    let vault = SecretsVault::new(&[7u8; 32]);
    for (key, value) in tenant_secrets {
        vault.cache_secret("default", key, (*value).to_string());
    }
    resolve_secret_templates(&integration.config, &vault, "default")
}

/// One run: Trigger, the packaged module's answer to it, and that answer
/// applied as TriggerComplete. Returns what TriggerComplete carried.
async fn run_once(server: &ServerState, run: u32) -> Value {
    run_once_with_secrets(server, run, &[]).await
}

async fn run_once_with_secrets(
    server: &ServerState,
    run: u32,
    tenant_secrets: &[(&str, &str)],
) -> Value {
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
        integration_config: trigger_config(tenant_secrets),
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
async fn a_whitespace_only_template_preserves_the_saved_message() {
    assert_whitespace_only_template_preserves_the_saved_message("user_message_template").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_odata_whitespace_only_template_preserves_the_saved_message() {
    assert_whitespace_only_template_preserves_the_saved_message("UserMessageTemplate").await;
}

async fn assert_whitespace_only_template_preserves_the_saved_message(template_field: &str) {
    for message_field in ["user_message", "UserMessage"] {
        let mut fields = json!({
            "name": "weekly digest",
            "schedule": "*/10 * * * *",
            "model": "gpt-5.5",
            "provider": "openai_codex",
        });
        fields[template_field] = json!(" \t\n");
        fields[message_field] = json!(MESSAGE);
        let server = server_with_job(fields).await;
        for run in 1..=2 {
            let completed = run_once(&server, run).await;
            assert_eq!(completed["user_message"], MESSAGE, "run {run}");
            assert_eq!(job(&server).await["fields"]["user_message"], MESSAGE);
            assert_eq!(spawned_session(&server).await["user_message"], MESSAGE);
        }
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
async fn a_job_with_no_model_runs_on_the_tenants_current_default() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "user_message": MESSAGE,
    }))
    .await;
    run_once_with_secrets(
        &server,
        1,
        &[("llm_model", "gpt-5.5"), ("llm_provider", "openai_codex")],
    )
    .await;
    // A Session with no model stops in context_preparer before its first turn.
    let session = spawned_session(&server).await;
    assert_eq!(session["model"], "gpt-5.5");
    assert_eq!(session["provider"], "openai_codex");

    // The run wrote the default back into the job; the next run still follows
    // the tenant's default when it changes.
    run_once_with_secrets(
        &server,
        2,
        &[
            ("llm_model", "claude-sonnet-5-5"),
            ("llm_provider", "anthropic"),
        ],
    )
    .await;
    let session = spawned_session(&server).await;
    assert_eq!(session["model"], "claude-sonnet-5-5");
    assert_eq!(session["provider"], "anthropic");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_jobs_own_model_stays_its_own_across_runs() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "user_message": MESSAGE,
        "model": "claude-sonnet-5-5",
        "provider": "anthropic",
    }))
    .await;
    let defaults = [("llm_model", "gpt-5.5"), ("llm_provider", "openai_codex")];
    for run in 1..=2 {
        run_once_with_secrets(&server, run, &defaults).await;
        let session = spawned_session(&server).await;
        assert_eq!(session["model"], "claude-sonnet-5-5", "run {run}");
        assert_eq!(session["provider"], "anthropic", "run {run}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_created_with_odata_property_names_keeps_its_message_and_model() {
    // The CSDL names CronJob's properties in PascalCase, and an OData create
    // stores them under those names next to the spec's snake_case fields.
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "UserMessage": MESSAGE,
        "Model": "claude-sonnet-5-5",
        "Provider": "anthropic",
    }))
    .await;
    let defaults = [("llm_model", "gpt-5.5"), ("llm_provider", "openai_codex")];
    run_once_with_secrets(&server, 1, &defaults).await;
    let session = spawned_session(&server).await;
    assert_eq!(session["user_message"], MESSAGE);
    assert_eq!(session["model"], "claude-sonnet-5-5");
    assert_eq!(session["provider"], "anthropic");

    // An OData PATCH after that run changes the PascalCase fields only, while
    // the run wrote its settings into the snake_case ones.
    server
        .update_tenant_entity_fields(
            &tenant(),
            "CronJob",
            JOB,
            json!({"UserMessage": "Write the monthly digest instead.", "Model": "gpt-5.5"}),
            false,
        )
        .await
        .unwrap();
    run_once_with_secrets(&server, 2, &defaults).await;
    let session = spawned_session(&server).await;
    assert_eq!(session["user_message"], "Write the monthly digest instead.");
    assert_eq!(session["model"], "gpt-5.5");
    assert_eq!(session["provider"], "anthropic");
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_odata_user_message_with_an_empty_string_clears_the_next_runs_message() {
    assert_clearing_odata_user_message_clears_the_next_runs_message(json!("")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_odata_user_message_with_null_clears_the_next_runs_message() {
    assert_clearing_odata_user_message_clears_the_next_runs_message(Value::Null).await;
}

async fn assert_clearing_odata_user_message_clears_the_next_runs_message(cleared: Value) {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "UserMessage": MESSAGE,
    }))
    .await;
    let defaults = [("llm_model", "gpt-5.5"), ("llm_provider", "openai_codex")];
    run_once_with_secrets(&server, 1, &defaults).await;
    assert_eq!(spawned_session(&server).await["user_message"], MESSAGE);

    // OData PATCH clears the authoritative field, leaving the last run's copy.
    server
        .update_tenant_entity_fields(
            &tenant(),
            "CronJob",
            JOB,
            json!({"UserMessage": cleared}),
            false,
        )
        .await
        .unwrap();
    assert_eq!(job(&server).await["fields"]["user_message"], MESSAGE);

    for run in 2..=3 {
        let completed = run_once_with_secrets(&server, run, &defaults).await;
        assert_eq!(
            completed["user_message"], "",
            "cleared with {cleared}, run {run}"
        );
        assert_eq!(job(&server).await["fields"]["user_message"], "");
        assert_eq!(spawned_session(&server).await["user_message"], "");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_odata_model_settings_with_empty_strings_uses_tenant_defaults() {
    assert_clearing_odata_model_settings_uses_tenant_defaults(json!("")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_odata_model_settings_with_null_uses_tenant_defaults() {
    assert_clearing_odata_model_settings_uses_tenant_defaults(Value::Null).await;
}

async fn assert_clearing_odata_model_settings_uses_tenant_defaults(cleared: Value) {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "UserMessage": MESSAGE,
        "Model": "claude-sonnet-5-5",
        "Provider": "anthropic",
    }))
    .await;
    let defaults = [("llm_model", "gpt-5.5"), ("llm_provider", "openai_codex")];
    run_once_with_secrets(&server, 1, &defaults).await;
    let session = spawned_session(&server).await;
    assert_eq!(session["model"], "claude-sonnet-5-5");
    assert_eq!(session["provider"], "anthropic");

    // OData PATCH merges the cleared authoritative fields, leaving the
    // previous run's snake_case copies in place.
    server
        .update_tenant_entity_fields(
            &tenant(),
            "CronJob",
            JOB,
            json!({"Model": cleared, "Provider": cleared}),
            false,
        )
        .await
        .unwrap();
    let fields = job(&server).await["fields"].clone();
    assert_eq!(fields["model"], "claude-sonnet-5-5");
    assert_eq!(fields["provider"], "anthropic");
    run_once_with_secrets(&server, 2, &defaults).await;
    let session = spawned_session(&server).await;
    assert_eq!(session["model"], "gpt-5.5", "cleared with {cleared}");
    assert_eq!(
        session["provider"], "openai_codex",
        "cleared with {cleared}"
    );

    // Clearing also restores following later changes to tenant defaults.
    let changed_defaults = [("llm_model", "gpt-6.1-sol"), ("llm_provider", "openai")];
    run_once_with_secrets(&server, 3, &changed_defaults).await;
    let session = spawned_session(&server).await;
    assert_eq!(session["model"], "gpt-6.1-sol", "cleared with {cleared}");
    assert_eq!(session["provider"], "openai", "cleared with {cleared}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_with_no_model_anywhere_still_completes_its_run() {
    let server = server_with_job(json!({
        "name": "weekly digest",
        "schedule": "*/10 * * * *",
        "user_message": MESSAGE,
    }))
    .await;
    // A failed Trigger stops the job's schedule, so a run with nothing to
    // resolve completes with no model and its Session reports why.
    let completed = run_once(&server, 1).await;
    assert_eq!(completed["model"], "");
    assert_eq!(completed["provider"], "");
    assert_eq!(completed["user_message"], MESSAGE);
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
