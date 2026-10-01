//! Scoped provider budgets, sequential retry, and trusted progress identity.
use super::*;
use std::sync::Arc;
use temper_runtime::scheduler::SimActorHandler;
use temper_server::entity_actor::sim_handler::EntityActorHandler;

fn source() -> String {
    let path = std::env::var_os("SESSION_PROFILE_SPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../os-apps/paw-agent/specs/session.ioa.toml")
        });
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn profile_budget_is_explicit_and_standard_budget_is_preserved() {
    let spec: toml::Value = toml::from_str(&source()).unwrap();
    let actions = spec["action"].as_array().unwrap();
    for (name, seconds, budget, failure) in [
        ("CallProviderStandard", "600", "600000", "Fail"),
        (
            "CallProviderForesight",
            "180",
            "180000",
            "ForesightProviderFailed",
        ),
        (
            "CallProviderForesightRetry",
            "180",
            "180000",
            "ForesightProviderRetryFailed",
        ),
    ] {
        let action = actions
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .expect(name);
        let trigger = &action["triggers"][0];
        assert_eq!(trigger["module"].as_str(), Some("provider_caller"));
        assert_eq!(trigger["config"]["timeout_secs"].as_str(), Some(seconds));
        assert_eq!(
            trigger["config"]["provider_caller_budget_ms"].as_str(),
            Some(budget)
        );
        assert_eq!(trigger["on_failure"].as_str(), Some(failure));
    }
}

#[test]
fn fast_attempts_reject_live_resume_and_stale_callbacks() {
    let mut session = EntityActorHandler::new(
        "Session",
        "session-profile",
        Arc::new(temper_jit::TransitionTable::from_ioa_source(&source())),
    );
    session.init().unwrap();
    session
        .handle_message(
            "Configure",
            r#"{"provider_latency_profile":"foresight_first_pass"}"#,
        )
        .unwrap();
    session.handle_message("ProvisionWorkspace", "{}").unwrap();
    session.handle_message("SandboxReady", "{}").unwrap();
    session
        .handle_message("ContextReadyAuthSkipped", "{}")
        .unwrap();
    let first = session
        .handle_message("CallProviderForesight", "{}")
        .unwrap();
    assert_eq!(first["counters"]["provider_call_attempt"], 1);
    assert!(session.handle_message("ResumeProvider", "{}").is_err());
    assert!(
        session
            .handle_message("CallProviderForesight", "{}")
            .is_err()
    );
    let failed = session
        .handle_message("ForesightProviderFailed", r#"{"error_message":"deadline"}"#)
        .unwrap();
    assert_eq!(failed["counters"]["foresight_provider_retry_count"], 1);
    session
        .handle_message("RetryForesightProvider", "{}")
        .unwrap();
    session.handle_message("ProviderAuthReady", "{}").unwrap();
    let retried = session
        .handle_message("CallProviderForesightRetry", "{}")
        .unwrap();
    assert_eq!(retried["counters"]["provider_call_attempt"], 2);
    assert!(
        session
            .handle_message(
                "ForesightProviderFailed",
                r#"{"error_message":"late first failure"}"#
            )
            .is_err()
    );
    assert!(
        session
            .handle_message(
                "ForesightProviderResponseReady",
                r#"{"expected_provider_attempt":1}"#
            )
            .is_err()
    );
    assert!(
        session
            .handle_message(
                "ForesightProviderAuthExpired",
                r#"{"expected_provider_attempt":1}"#
            )
            .is_err()
    );
    session
        .handle_message(
            "ForesightProviderResponseReady",
            r#"{"expected_provider_attempt":2}"#,
        )
        .unwrap();
    assert_eq!(session.current_status(), "ApplyingProviderResponse");
    assert!(
        session
            .handle_message(
                "ForesightProviderRetryFailed",
                r#"{"error_message":"late failure"}"#
            )
            .is_err()
    );
}

#[test]
fn internal_provider_entries_are_not_dashboard_or_session_capabilities() {
    use temper_authz::{AuthzEngine, PrincipalKind, SecurityContext};
    let policy = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../os-apps/paw-agent/policies/session.cedar"),
    )
    .unwrap();
    let authz = AuthzEngine::new(&policy).unwrap();
    let system = AgentContext::for_service("system").security_ctx.unwrap();
    let relay = AgentContext::for_service("wasm-runtime")
        .security_ctx
        .unwrap();
    let mut admin = SecurityContext::from_resolved_identity("dashboard", "human", None);
    admin.principal.kind = PrincipalKind::Admin;
    let attrs = std::collections::HashMap::from([("id".into(), json!("session-profile"))]);
    for action in [
        "CallProviderStandard",
        "CallProviderForesight",
        "CallProviderForesightRetry",
        "RetryForesightProvider",
        "ForesightProviderFailed",
        "ForesightProviderRetryFailed",
        "ForesightProviderResponseReady",
        "ForesightProviderAuthExpired",
    ] {
        assert!(
            authz
                .authorize(&system, action, "Session", &attrs)
                .is_allowed(),
            "{action}"
        );
        for caller in [&relay, &admin] {
            assert!(
                !authz
                    .authorize(caller, action, "Session", &attrs)
                    .is_allowed(),
                "{}:{action}",
                caller.principal.id
            );
        }
    }
    assert!(
        authz
            .authorize(&system, "ProgressMade", "Session", &attrs)
            .is_allowed()
    );
    assert!(
        !authz
            .authorize(&relay, "ProgressMade", "Session", &attrs)
            .is_allowed()
    );
}

fn callback_module(callback: &str) -> Vec<u8> {
    let output = json!({"success":true,"action":callback,"params":{}}).to_string();
    let data = output
        .bytes()
        .map(|byte| format!("\\{byte:02x}"))
        .collect::<String>();
    format!(
        r#"(module
      (import "env" "host_set_result" (func $set (param i32 i32)))
      (memory (export "memory") 2)
      (data (i32.const 65536) "{data}")
      (func (export "run") (param i32 i32) (result i32)
        i32.const 65536 i32.const {} call $set i32.const 0))"#,
        output.len()
    )
    .into_bytes()
}

async fn runtime() -> ServerState {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let xml = std::fs::read_to_string(root.join("os-apps/paw-agent/specs/model.csdl.xml")).unwrap();
    // Start at the context callback boundary; provisioning, response application,
    // and terminal delivery are out of scope. Production entry/retry actions,
    // guards, policy, provider module, and HTTP progress dispatch stay real.
    let spec = source().replacen("initial = \"Created\"", "initial = \"PreparingContext\"", 1)
        .replace("{secret:temper_api_url}", "http://127.0.0.1:3000")
        .replace("provider_initial_heartbeat_enabled = \"false\"", "provider_initial_heartbeat_enabled = \"false\"\nprovider_progress_dispatch_enabled = \"true\"");
    let mut registry = SpecRegistry::new();
    registry.register_tenant(
        "default",
        temper_spec::csdl::parse_csdl(&xml).unwrap(),
        xml,
        &[("Session", &spec)],
    );
    registry.set_verification_status(
        &TenantId::default(),
        "Session",
        VerificationStatus::Completed(EntityVerificationResult {
            all_passed: true,
            levels: vec![],
            verified_at: "2026-09-16T00:00:00Z".into(),
        }),
    );
    let state = ServerState::from_registry(ActorSystem::new("provider-profile"), registry);
    state.rebuild_reaction_dispatcher();
    state
        .authz
        .reload_tenant_policies(
            "default",
            &std::fs::read_to_string(root.join("os-apps/paw-agent/policies/session.cedar"))
                .unwrap(),
        )
        .unwrap();
    let path = if let Some(path) = std::env::var_os("PROVIDER_WASM_FILE") {
        PathBuf::from(path)
    } else {
        let output = std::process::Command::new("bash")
            .current_dir(&root)
            .args(["-c", "set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm os-apps/paw-agent/wasm/provider_caller wasm32-unknown-unknown --locked"])
            .output().expect("run canonical provider WASM build");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    };
    let modules = [
        ("provider_caller", std::fs::read(path).unwrap()),
        ("provider_auth_gate", callback_module("ProviderAuthReady")),
        ("provider_response_applier", callback_module("")),
        ("agent_reply", callback_module("")),
        ("sandbox_provisioner", callback_module("")),
        ("emit_ots_trajectory", callback_module("")),
    ];
    for (name, bytes) in modules {
        let hash = state.wasm_engine.compile_and_cache(&bytes).unwrap();
        state
            .wasm_module_registry
            .write()
            .unwrap()
            .register_builtin(name, &hash);
    }
    state
}

async fn wait_status(
    state: &ServerState,
    id: &str,
    status: &str,
) -> temper_server::entity_actor::EntityState {
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let result = state
                .get_tenant_entity_state(&TenantId::default(), "Session", id)
                .await
                .unwrap();
            if result.state.status == status {
                return result.state;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    match result {
        Ok(state) => state,
        Err(_) => {
            let actual = state
                .get_tenant_entity_state(&TenantId::default(), "Session", id)
                .await
                .unwrap();
            panic!("{id} did not reach {status}: {:?}", actual.state);
        }
    }
}

async fn enter(state: &ServerState, id: &str, profile: &str, provider: &str) {
    let prepared = json!({"version":1,"messages":[{"role":"user","content":"hello"}],"tools":[],"system_prompt":"","system_prompt_hash":"","system_prompt_file_id":"","conversation_file_id":"","session_file_id":"","session_leaf_id":"","workspace_id":"","use_session_tree":false,"context_tokens":1,"context_bytes":5,"entries_loaded":1,"content_files_loaded":0});
    state
        .get_or_create_tenant_entity(
            &TenantId::default(),
            "Session",
            id,
            json!({"provider_latency_profile":profile,"provider":provider,"model":"test-model"}),
        )
        .await
        .unwrap();
    let result = state
        .dispatch(DispatchCommand {
            tenant: &TenantId::default(),
            entity_type: "Session",
            entity_id: id,
            action: "ContextReadyAuthSkipped",
            params: json!({"prepared_context_inline_json":prepared.to_string()}),
            agent_ctx: &AgentContext::for_service("wasm-runtime"),
            await_integration: true,
            await_reactions: true,
        })
        .await
        .unwrap();
    assert!(result.success, "{result:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn declared_entry_permits_real_provider_progress_and_routes_each_profile() {
    let state = runtime().await;
    for (id, profile, entry, callback) in [
        (
            "standard",
            "standard",
            "CallProviderStandard",
            "ProviderResponseReady",
        ),
        (
            "legacy",
            "",
            "CallProviderStandard",
            "ProviderResponseReady",
        ),
        (
            "fast",
            "foresight_first_pass",
            "CallProviderForesight",
            "ForesightProviderResponseReady",
        ),
    ] {
        enter(&state, id, profile, "mock").await;
        let final_state = wait_status(&state, id, "ApplyingProviderResponse").await;
        assert_eq!(
            final_state
                .events
                .iter()
                .filter(|event| event.action == entry)
                .count(),
            1,
            "{final_state:?}"
        );
        assert!(
            final_state
                .events
                .iter()
                .any(|event| event.action == callback),
            "{final_state:?}"
        );
        assert!(
            final_state
                .counters
                .get("progress_token")
                .copied()
                .unwrap_or(0)
                >= 2,
            "actual provider boundary ProgressMade writes must be authorized: {final_state:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn completed_failure_retries_once_using_the_committed_counter_then_fails() {
    let state = runtime().await;
    enter(
        &state,
        "two-failures",
        "foresight_first_pass",
        "unsupported-fixture-provider",
    )
    .await;
    let final_state = wait_status(&state, "two-failures", "Failed").await;
    for action in [
        "CallProviderForesight",
        "ForesightProviderFailed",
        "RetryForesightProvider",
        "ProviderAuthReady",
        "CallProviderForesightRetry",
        "ForesightProviderRetryFailed",
        "Fail",
    ] {
        assert_eq!(
            final_state
                .events
                .iter()
                .filter(|event| event.action == action)
                .count(),
            1,
            "{action}: {final_state:?}"
        );
    }
    assert_eq!(final_state.counters["foresight_provider_retry_count"], 1);
    assert_eq!(final_state.counters["provider_call_attempt"], 2);
}

/// Exercise the actual guest HTTP stream path, without external credentials.
struct ProviderErrorHost {
    response: Option<(u16, String)>,
    requests: std::sync::atomic::AtomicUsize,
    delivered: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl temper_wasm::WasmHost for ProviderErrorHost {
    async fn http_call(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &str,
    ) -> Result<(u16, String), String> {
        Err("unexpected non-stream request".into())
    }
    async fn http_call_binary(
        &self,
        _: &str,
        _: &str,
        _: &[(String, String)],
        _: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        Err("unexpected binary request".into())
    }
    fn get_secret(&self, _: &str) -> Result<String, String> {
        Err("unexpected secret read".into())
    }
    fn log(&self, _: &str, _: &str) {}
    async fn http_stream_begin_outbound(
        &self,
        request: temper_wasm::http_stream::HttpRequestHead,
    ) -> Result<temper_wasm::http_stream::HttpStreamHandles, String> {
        use temper_wasm::http_stream::{HttpStreamHandles, StreamHandle};
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.delivered
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if self.response.is_none() {
            return Err("fixture transport unavailable".into());
        }
        Ok(HttpStreamHandles {
            request_body: StreamHandle(1),
            response_body: StreamHandle(2),
        })
    }
    async fn http_stream_try_write(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
        chunk: Vec<u8>,
    ) -> Result<usize, temper_wasm::http_stream::StreamError> {
        Ok(chunk.len())
    }
    async fn http_stream_close(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<(), temper_wasm::http_stream::StreamError> {
        Ok(())
    }
    async fn http_stream_response_head(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<temper_wasm::http_stream::HttpResponseHead, String> {
        Ok(temper_wasm::http_stream::HttpResponseHead {
            status: self.response.as_ref().unwrap().0,
            headers: vec![],
        })
    }
    async fn http_stream_read(
        &self,
        _: temper_wasm::http_stream::StreamHandle,
    ) -> Result<Vec<u8>, temper_wasm::http_stream::StreamError> {
        if self
            .delivered
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(vec![]);
        }
        Ok(self.response.as_ref().unwrap().1.as_bytes().to_vec())
    }
}

async fn invoke_provider_error(
    status: u16,
    response: Option<serde_json::Value>,
) -> (usize, String) {
    use std::collections::BTreeMap;
    use std::sync::{OnceLock, RwLock};
    use temper_wasm::{StreamRegistry, WasmEngine, WasmInvocationContext, WasmResourceLimits};
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let bytes = std::fs::read(
        std::env::var_os("PROVIDER_WASM_FILE").expect("explicit frozen WASM required"),
    )
    .unwrap();
    let hash = engine.compile_and_cache(&bytes).unwrap();
    let host = Arc::new(ProviderErrorHost {
        response: response.map(|v| {
            (
                status,
                if status == 200 {
                    format!("data: {v}\n\n")
                } else {
                    v.to_string()
                },
            )
        }),
        requests: 0.into(),
        delivered: false.into(),
    });
    let prepared = json!({"version":1,"messages":[{"role":"user","content":"hello"}],"tools":[],"system_prompt":"","system_prompt_hash":"","system_prompt_file_id":"","conversation_file_id":"","session_file_id":"","session_leaf_id":"","workspace_id":"","use_session_tree":false,"context_tokens":1,"context_bytes":5,"entries_loaded":1,"content_files_loaded":0});
    let ctx = WasmInvocationContext {
        tenant: "default".into(),
        entity_type: "Session".into(),
        entity_id: "429-test".into(),
        trigger_action: "CallProviderStandard".into(),
        wasm_module: Some("provider_caller".into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"CallingProvider","fields":{"provider":"openai_codex","model":"fixture-model","prepared_context_inline_json":prepared.to_string()}}),
        integration_config: BTreeMap::from([
            ("temper_api_url".into(), "https://temper.test".into()),
            (
                "openai_codex_access_token".into(),
                "fixture-not-a-credential".into(),
            ),
            ("openai_codex_account_id".into(), "fixture-account".into()),
        ]),
        agent_id: None,
        session_id: None,
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    };
    let result = engine
        .invoke(
            &hash,
            &ctx,
            host.clone(),
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap();
    assert!(!result.success, "{result:?}");
    (
        host.requests.load(std::sync::atomic::Ordering::SeqCst),
        result.error.unwrap_or_default(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_usage_limit_returns_one_http_429_with_bounded_provider_details() {
    let (requests,error) = invoke_provider_error(429, Some(json!({"error":{"type":"usage_limit_reached","message":"Usage exhausted; try after reset.","resets_at":1790200000,"resets_in_seconds":604800,"account_id":"must-not-leak","unknown":"must-not-leak"},"access_token":"must-not-leak"}))).await;
    assert_eq!((requests, error.contains("HTTP 429")), (1, true), "{error}");
    for field in [
        "usage_limit_reached",
        "Usage exhausted; try after reset.",
        "1790200000",
        "604800",
    ] {
        assert!(error.contains(field), "{error}");
    }
    assert!(!error.contains("must-not-leak"), "{error}");
    assert!(
        !error.contains("before a provider HTTP response"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_429_preserves_attempt_budget_and_reports_http_response() {
    let (requests,error) = invoke_provider_error(429, Some(json!({"error":{"type":"rate_limit_error","code":"rate_limit_exceeded","message":"Temporary rate limit"}}))).await;
    assert_eq!(requests, 5, "{error}");
    assert!(error.contains("HTTP 429"), "{error}");
    assert!(error.contains("Temporary rate limit"), "{error}");
    assert!(
        !error.contains("before a provider HTTP response"),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exhausted_transport_failure_does_not_claim_an_http_response() {
    let (requests, error) = invoke_provider_error(0, None).await;
    assert_eq!(requests, 5, "{error}");
    assert!(error.contains("before a provider HTTP response"), "{error}");
    assert!(error.contains("streaming_call begin"), "{error}");
    assert!(!error.contains("HTTP 429"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_response_stream_keeps_the_received_http_status() {
    let (requests, error) = invoke_provider_error(
        200,
        Some(json!({"type":"response.failed","response":{"error":{"code":"server_error"}}})),
    )
    .await;
    assert_eq!(requests, 5, "{error}");
    assert!(error.contains("HTTP 200"), "{error}");
    assert!(error.contains("response.failed"), "{error}");
    assert!(
        !error.contains("before a provider HTTP response"),
        "{error}"
    );
}
