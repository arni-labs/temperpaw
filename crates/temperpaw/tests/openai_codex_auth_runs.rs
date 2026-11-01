//! OpenAICodexAuth sign-in under the real spec, an in-process Temper server and
//! the packaged openai_codex_auth module, with OpenAI answered by a simulated
//! host. The server's own WASM host always makes real requests, so the server
//! holds the spec's moves without their triggers and this test runs the module
//! each move declares, then dispatches the callback it names, the way the
//! server chains them inline. A path only passes through moves the spec
//! declares.
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use temper_runtime::{ActorSystem, tenant::TenantId};
use temper_server::{
    registry::SpecRegistry,
    request_context::AgentContext,
    state::{DispatchCommand, ServerState},
};
use temper_spec::automaton::{Automaton, TriggerKind};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmInvocationContext, WasmInvocationResult,
    WasmResourceLimits,
};

const AUTH: &str = "openai-codex-auth";
const USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
/// The time a person needs to open the link and type the code.
const MIN_LEFT_MS: i64 = 30_000;

fn app() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-agent")
}

fn tenant() -> TenantId {
    TenantId::from("default".to_owned())
}

fn spec() -> Automaton {
    let ioa = std::fs::read_to_string(app().join("specs/openai_codex_auth.ioa.toml")).unwrap();
    temper_spec::automaton::parse_automaton(&ioa).unwrap()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The spec without its `[[action.triggers]]` blocks, so the server runs no
/// module of its own.
fn ioa_without_triggers(ioa: &str) -> String {
    let mut out = String::new();
    let mut in_trigger = false;
    for line in ioa.lines() {
        let header = line.trim_start();
        if header.starts_with("[[action.triggers]]") {
            in_trigger = true;
            continue;
        }
        if in_trigger && header.starts_with("[[") {
            in_trigger = false;
        }
        if !in_trigger {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

async fn server() -> ServerState {
    let xml = std::fs::read_to_string(app().join("specs/model.csdl.xml")).unwrap();
    let full = std::fs::read_to_string(app().join("specs/openai_codex_auth.ioa.toml")).unwrap();
    let ioa = ioa_without_triggers(&full);
    let held = temper_spec::automaton::parse_automaton(&ioa).unwrap();
    let mut moves = spec().actions;
    for action in &mut moves {
        action.triggers.clear();
    }
    assert_eq!(
        serde_json::to_value(&held.actions).unwrap(),
        serde_json::to_value(&moves).unwrap(),
        "only the triggers are left out"
    );
    assert_eq!(
        serde_json::to_value(&held.state_timeouts).unwrap(),
        serde_json::to_value(spec().state_timeouts).unwrap()
    );
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("OpenAICodexAuth", ioa.as_str())],
    );
    let server = ServerState::from_registry(ActorSystem::new("openai-codex-auth-runs"), registry);
    server
        .get_or_create_tenant_entity(&tenant(), "OpenAICodexAuth", AUTH, json!({ "id": AUTH }))
        .await
        .unwrap();
    server
}

async fn act(server: &ServerState, action: &str, params: Value) {
    let tenant = tenant();
    let agent = AgentContext::for_service("openai-codex-auth-runs-test");
    let response = server
        .dispatch(DispatchCommand {
            tenant: &tenant,
            entity_type: "OpenAICodexAuth",
            entity_id: AUTH,
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

async fn auth(server: &ServerState) -> Value {
    let row = server
        .get_tenant_entity_state(&tenant(), "OpenAICodexAuth", AUTH)
        .await
        .unwrap();
    serde_json::to_value(&row.state).unwrap()
}

/// OpenAI as the module sees it: only the listed URLs answer.
fn openai(responses: &[(&str, u16, Value)]) -> SimWasmHost {
    responses.iter().fold(
        SimWasmHost::new().with_default_response(500, "unexpected request"),
        |host, (url, status, body)| host.with_response(url, *status, &body.to_string()),
    )
}

fn new_code() -> (&'static str, u16, Value) {
    (
        USER_CODE_URL,
        200,
        json!({ "device_auth_id": "device-new", "user_code": "NEW-CODE", "interval": 5 }),
    )
}

/// Runs the module `action` declares against the entity as it is now, or
/// None when the action declares no module.
async fn run_module(
    server: &ServerState,
    action: &str,
    openai: Arc<SimWasmHost>,
) -> Option<WasmInvocationResult> {
    let spec = spec();
    let trigger = spec
        .actions
        .iter()
        .find(|declared| declared.name == action)?
        .triggers
        .iter()
        .find(|trigger| trigger.kind == TriggerKind::Wasm)?;
    let context = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "OpenAICodexAuth".into(),
        entity_id: AUTH.into(),
        trigger_action: action.into(),
        wasm_module: trigger.module.clone(),
        trigger_params: json!({}),
        entity_state: auth(server).await,
        agent_id: None,
        session_id: None,
        integration_config: trigger.config.clone(),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
        trigger_principal: None,
    };
    let bytes = std::fs::read(app().join("wasm/openai_codex_auth/openai_codex_auth.wasm"))
        .unwrap_or_else(|error| panic!("Build paw-agent WASMs before this proof: {error}"));
    let engine = WasmEngine::new().unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let result = engine
        .invoke(
            &hash,
            &context,
            openai,
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    Some(result)
}

/// Dispatches `action`, then runs the module each action declares and
/// dispatches the callback it names, until an action runs no module. Every
/// time the entity enters DeviceCodeReady its code must leave a person time to
/// enter it. Returns the entity at the end.
async fn settle(server: &ServerState, action: &str, openai: SimWasmHost) -> Value {
    let openai = Arc::new(openai);
    let mut next = Some((action.to_string(), json!({})));
    for _ in 0..8 {
        let Some((action, params)) = next.take() else {
            return auth(server).await;
        };
        act(server, &action, params).await;
        let state = auth(server).await;
        if state["status"] == "DeviceCodeReady" {
            let expires_at_ms: i64 = state["fields"]["expires_at_ms"]
                .as_str()
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| panic!("{action} left DeviceCodeReady with no expiry: {state}"));
            assert!(
                expires_at_ms - now_ms() > MIN_LEFT_MS,
                "{action} left DeviceCodeReady holding a code with no time to enter it: {state}"
            );
        }
        let Some(result) = run_module(server, &action, openai.clone()).await else {
            continue;
        };
        assert!(result.success, "{action}'s module failed: {result:?}");
        if !result.callback_action.is_empty() {
            next = Some((result.callback_action, result.callback_params));
        }
    }
    panic!("the {action} chain did not settle");
}

async fn holding_a_code(server: &ServerState, user_code: &str, expires_at_ms: i64) {
    act(server, "StartDeviceLogin", json!({})).await;
    act(
        server,
        "DeviceCodeReady",
        json!({
            "verification_url": "https://auth.openai.com/codex/device",
            "user_code": user_code,
            "device_auth_id": "device-old",
            "poll_interval_ms": "5000",
            "expires_at_ms": expires_at_ms.to_string(),
        }),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_poll_of_a_code_with_no_time_left_gets_a_new_code() {
    for (case, left_ms) in [("ran out a minute ago", -60_000), ("has 20 s left", 20_000)] {
        let server = server().await;
        holding_a_code(&server, "OLD-CODE", now_ms() + left_ms).await;
        let after = settle(
            &server,
            "PollDeviceLogin",
            openai(&[
                (
                    DEVICE_TOKEN_URL,
                    403,
                    json!({ "error": "authorization_pending" }),
                ),
                new_code(),
            ]),
        )
        .await;
        assert_eq!(after["status"], "DeviceCodeReady", "{case}: {after}");
        assert_eq!(after["fields"]["user_code"], "NEW-CODE", "{case}: {after}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_poll_of_a_code_with_time_left_keeps_it() {
    let server = server().await;
    let expires_at_ms = now_ms() + 10 * 60_000;
    holding_a_code(&server, "OLD-CODE", expires_at_ms).await;
    let after = settle(
        &server,
        "PollDeviceLogin",
        openai(&[(
            DEVICE_TOKEN_URL,
            403,
            json!({ "error": "authorization_pending" }),
        )]),
    )
    .await;
    assert_eq!(after["status"], "DeviceCodeReady", "{after}");
    assert_eq!(after["fields"]["user_code"], "OLD-CODE");
    assert_eq!(after["fields"]["expires_at_ms"], expires_at_ms.to_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_openai_refuses_gets_a_new_code() {
    for (case, status, body) in [
        (
            "an expired refresh token",
            401,
            json!({ "error": { "code": "refresh_token_expired" } }),
        ),
        (
            "a revoked refresh token",
            401,
            json!({ "error": { "code": "refresh_token_invalidated" } }),
        ),
        ("an invalid grant", 400, json!({ "error": "invalid_grant" })),
    ] {
        let server = server().await;
        let after = settle(
            &server,
            "EnsureFresh",
            openai(&[(REFRESH_URL, status, body), new_code()])
                .with_secret("openai_codex_refresh_token", "refresh-old"),
        )
        .await;
        assert_eq!(after["status"], "DeviceCodeReady", "{case}: {after}");
        assert_eq!(after["fields"]["user_code"], "NEW-CODE", "{case}: {after}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_with_no_refresh_token_gets_a_code_even_after_a_disconnect() {
    let never_signed_in = server().await;
    let after = settle(&never_signed_in, "EnsureFresh", openai(&[new_code()])).await;
    assert_eq!(
        after["status"], "DeviceCodeReady",
        "never signed in: {after}"
    );

    let disconnected = server().await;
    act(&disconnected, "Disconnect", json!({})).await;
    let after = settle(&disconnected, "EnsureFresh", openai(&[new_code()])).await;
    assert_eq!(
        after["status"], "DeviceCodeReady",
        "after a disconnect: {after}"
    );
    assert_eq!(after["fields"]["user_code"], "NEW-CODE");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_openai_could_not_answer_is_not_taken_for_a_dead_login() {
    let server = server().await;
    act(&server, "EnsureFresh", json!({})).await;
    let openai = openai(&[(REFRESH_URL, 503, json!({ "error": "unavailable" }))])
        .with_secret("openai_codex_refresh_token", "refresh-old");
    let result = run_module(&server, "EnsureFresh", Arc::new(openai))
        .await
        .unwrap();
    assert!(!result.success, "{result:?}");
    assert_ne!(result.callback_action, "SignInRequired");
}
