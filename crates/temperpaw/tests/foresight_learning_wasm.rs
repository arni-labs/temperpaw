//! Invoke real Foresight WASMs with deterministic HTTP fixtures at the host boundary.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
};
use temper_wasm::{
    SimWasmHost, StreamRegistry, WasmEngine, WasmHost, WasmInvocationContext, WasmInvocationResult,
    WasmResourceLimits,
};

fn module_bytes(module: &str) -> Vec<u8> {
    static MODULES: OnceLock<Mutex<BTreeMap<String, Vec<u8>>>> = OnceLock::new();
    let mut modules = MODULES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap();
    modules.entry(module.into()).or_insert_with(|| {
        let path = if let Some(dir) = std::env::var_os("FORESIGHT_WASM_DIR") {
            // Explicit directories let the same regression exercise frozen before/after bytes.
            PathBuf::from(dir).join(format!("{module}.wasm"))
        } else {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            let output = std::process::Command::new("bash")
                .current_dir(&root)
                .args(["-c", "set -euo pipefail; source os-apps/wasm-build-env.sh; temperpaw_build_wasm \"$1\" wasm32-unknown-unknown", "foresight-wasm-test"])
                .arg(root.join(format!("os-apps/paw-foresight/wasm/{module}")))
                .output().expect("run canonical WASM build helper");
            assert!(output.status.success(), "build {module}: {}", String::from_utf8_lossy(&output.stderr));
            PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
        };
        std::fs::read(&path)
            .unwrap_or_else(|e| panic!("Read built Foresight WASM {}: {e}", path.display()))
    }).clone()
}

fn context(module: &str, trigger: &str, fields: Value) -> WasmInvocationContext {
    WasmInvocationContext {
        tenant: "deep-sci-fi".into(),
        entity_type: "World".into(),
        entity_id: "world-1".into(),
        trigger_action: trigger.into(),
        wasm_module: Some(module.into()),
        trigger_params: json!({}),
        entity_state: json!({"status":"RegisteringForecasts","fields":fields,"counters":{"registration_attempt":1},"events":[{"action":trigger,"timestamp":"2026-09-16T04:00:00.123456Z"}]}),
        agent_id: None,
        session_id: None,
        integration_config: BTreeMap::from([(
            "temper_api_url".into(),
            "https://temper.test".into(),
        )]),
        trace_id: String::new(),
        workflow_root_entity_type: None,
        workflow_root_entity_id: None,
        workflow_run_id: None,
        http_request: None,
    }
}
async fn invoke(
    module: &str,
    mut ctx: WasmInvocationContext,
    host: impl WasmHost + 'static,
) -> WasmInvocationResult {
    // Each invocation still receives a fresh host/context; reuse only compiled
    // immutable modules so multi-case boundary tests do not compile per row.
    static ENGINE: OnceLock<WasmEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| WasmEngine::new().unwrap());
    let hash = engine.compile_and_cache(&module_bytes(module)).unwrap();
    let host = Arc::new(host);
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
    if result.callback_action != "RegistrationSnapshotPrepared" {
        return result;
    }
    // Follow the declared snapshot-commit edge with a later host event, using
    // the exact durable callback payload. Later live reads must not alter it.
    for (key, value) in result.callback_params.as_object().unwrap() {
        ctx.entity_state["fields"][key] = value.clone();
    }
    ctx.trigger_action = "CommitForecastSnapshot".into();
    ctx.entity_state["events"] =
        json!([{"action":"CommitForecastSnapshot","timestamp":"2026-09-16T04:00:01Z"}]);
    engine
        .invoke(
            &hash,
            &ctx,
            host,
            &WasmResourceLimits::default(),
            Arc::new(RwLock::new(StreamRegistry::default())),
        )
        .await
        .unwrap()
}
fn registration_host() -> SimWasmHost {
    SimWasmHost::new().with_default_response(404,"missing")
      .with_response("https://temper.test/tdata/Endpoints?$filter=world_id eq 'world-1'&$top=513",200,"{\"value\":[]}")
      .with_response("https://temper.test/tdata/EventNodes?$filter=world_id eq 'world-1'&$top=513",200,&json!({"value":[{"Id":"node-1","Status":"Proposed","Probability":"0.55","Provenance":"authored","ResolveBy":"2027-06-01T00:00:00Z","Statement":"Synthetic future event"}]}).to_string())
      .with_response("https://temper.test/tdata/Forecasts?$filter=event_node_id eq 'node-1'&$top=513",200,"{\"value\":[]}")
}
fn world(mode: &str) -> Value {
    json!({"learning_mode":mode,"frontier_date":"2028-01-01T00:00:00Z","last_ingest_date":"2026-09-16T00:00:00Z","model_json":"","adopted_learning_run_id":""})
}

#[tokio::test(flavor = "multi_thread")]
async fn observed_manual_questions_remain_eligible_but_unreviewed_worker_nodes_do_not() {
    for (author, expected) in [
        ("dashboard", "ForecastPrepared"),
        ("repairer", "ForecastRegistrationComplete"),
    ] {
        let host = registration_host().with_response("https://temper.test/tdata/EventNodes?$filter=world_id eq 'world-1'&$top=513",200,
            &json!({"value":[{"Id":"node-1","AuthorAgentId":author,"Status":"Proposed","Probability":"0.55","Provenance":"authored","ResolveBy":"2027-06-01T00:00:00Z","Statement":"Explicit question"}]}).to_string());
        let result = invoke(
            "register_forecasts",
            context(
                "register_forecasts",
                "StartForecastRegistration",
                world("observed"),
            ),
            host,
        )
        .await;
        assert_eq!(result.callback_action, expected, "{author}: {result:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn corridor_registration_waits_for_an_evaluated_path_and_excludes_siblings() {
    for status in [
        "Solving",
        "Repaired",
        "Challenged",
        "Scored",
        "Canonical",
        "Rejected",
    ] {
        let path = json!({"Id":"path-ready","WorldId":"world-1","Status":status,"RequiredNodeIds":"[\"node-1\"]"});
        let host = registration_host()
            .with_response("https://temper.test/tdata/Endpoints?$filter=world_id eq 'world-1'&$top=513",200,"{\"value\":[{\"Id\":\"endpoint\"}]}")
            .with_response("https://temper.test/tdata/Paths?$filter=world_id eq 'world-1'&$top=513",200,&json!({"value":[path]}).to_string())
            .with_response("https://temper.test/tdata/EventNodes?$filter=world_id eq 'world-1'&$top=513",200,&json!({"value":[{"Id":"unfinished-sibling","Status":"Proposed","Probability":"0.8","Provenance":"authored","ResolveBy":"2027-06-01T00:00:00Z","Statement":"Unverified sibling requirement"},{"Id":"node-1","Status":"Proposed","Probability":"0.55","Provenance":"authored","ResolveBy":"2027-06-01T00:00:00Z","Statement":"Evaluated requirement"}]}).to_string())
            .with_response("https://temper.test/tdata/Paths('path-ready')",200,&path.to_string());
        let result = invoke(
            "register_forecasts",
            context(
                "register_forecasts",
                "StartForecastRegistration",
                world("simulated"),
            ),
            host,
        )
        .await;
        assert!(result.success, "{result:?}");
        assert_eq!(
            result.callback_action,
            if status == "Canonical" {
                "ForecastPrepared"
            } else {
                "ForecastRegistrationComplete"
            },
            "{status}: {result:?}"
        );
        assert_eq!(result.callback_params["expected_registration_attempt"], 1);
        if status == "Canonical" {
            assert_eq!(result.callback_params["forecast_event_node_id"], "node-1");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn observed_registration_uses_host_event_time_even_without_ingest_date() {
    for supplied in ["", "2000-01-01T00:00:00Z"] {
        let mut fields = world("observed");
        fields["last_ingest_date"] = json!(supplied);
        let result = invoke(
            "register_forecasts",
            context("register_forecasts", "PathsScored", fields),
            registration_host()
                .with_response("https://temper.test/tdata/Endpoints?$filter=world_id eq 'world-1'&$top=513",200,"{\"value\":[{\"Id\":\"endpoint\"}]}")
                .with_response("https://temper.test/tdata/Paths?$filter=world_id eq 'world-1'&$top=513",200,"{\"value\":[{\"Id\":\"path\"}]}")
                .with_response("https://temper.test/tdata/Paths('path')",200,&json!({"Id":"path","WorldId":"world-1","Status":"Canonical","RequiredNodeIds":"[\"node-1\"]"}).to_string()),
        )
        .await;
        assert!(result.success, "{result:?}");
        assert_eq!(result.callback_action, "ForecastPrepared", "{result:?}");
        assert_eq!(
            result.callback_params["forecast_registered_at"],
            "2026-09-16T04:00:01Z"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn registration_input_and_lookup_failures_return_recoverable_callback() {
    for (date, host) in [
        ("", registration_host()),
        (
            "2026-09-16T00:00:00Z",
            SimWasmHost::new().with_default_response(503, "temporary"),
        ),
    ] {
        let mut fields = world("historical");
        fields["last_ingest_date"] = json!(date);
        let result = invoke(
            "register_forecasts",
            context("register_forecasts", "RegisterForecasts", fields),
            host,
        )
        .await;
        assert!(
            result.success,
            "failure is a declared recovery callback: {result:?}"
        );
        assert_eq!(result.callback_action, "ForecastRegistrationFailed");
        assert!(
            !result.callback_params["error_message"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn authoritative_registered_identity_advances_when_collection_lags() {
    let fields = world("simulated");
    let first = invoke(
        "register_forecasts",
        context("register_forecasts", "RegisterForecasts", fields.clone()),
        registration_host(),
    )
    .await;
    assert_eq!(first.callback_action, "ForecastPrepared", "{first:?}");
    let id = first.callback_params["forecast_id"].as_str().unwrap();
    let key = &first.callback_params["forecast_registration_key"];
    let existing =
        json!({"entity_id":id,"status":"Preregistered","fields":{"registration_key":key}});
    let host = registration_host().with_response(
        &format!("https://temper.test/tdata/Forecasts('{id}')"),
        200,
        &existing.to_string(),
    );
    let retry = invoke(
        "register_forecasts",
        context("register_forecasts", "RegisterForecasts", fields),
        host,
    )
    .await;
    assert_eq!(
        retry.callback_action, "ForecastRegistrationComplete",
        "{retry:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn historical_grading_scores_every_revision_but_requests_learning_once() {
    for evidence in ["eligible", "undated", "unverified", "proxy", "bad_source"] {
        let mut rows = vec![];
        let mut host = SimWasmHost::new().with_default_response(503, "unexpected request");
        for (id, event) in [("f1", "e1"), ("f1-revision", "e1"), ("f2", "e2")] {
            rows.push(json!({"Id":id,"Status":"Preregistered","EventNodeId":event,"Question":event,"Probability":"0.55","BaseProbability":"0.55","EvidenceKind":if evidence=="proxy"{"proxy"}else{"historical"},"RegisteredAt":"2025-03-01T00:00:00Z"}));
            host = host
                .with_response(
                    &format!("https://temper.test/tdata/Forecasts('{id}')/TemperPaw.Resolve"),
                    200,
                    "{}",
                )
                .with_response(
                    &format!("https://temper.test/tdata/Forecasts('{id}')/TemperPaw.ScoreBatch"),
                    200,
                    "{}",
                );
        }
        let actuals = json!([{"event_node_id":if evidence=="unverified"{""}else{"e1"},"question_contains":"e1","outcome":"yes","resolved_at":if evidence!="undated"{"2025-06-01T00:00:00Z"}else{""},"source_refs":[if evidence=="bad_source"{"ftp://invalid.test/actual"}else{"https://example.test/actual"}]},{"event_node_id":if evidence=="unverified"{""}else{"e2"},"question_contains":"e2","outcome":"no","resolved_at":if evidence!="undated"{"2025-06-02T00:00:00Z"}else{""},"source_refs":[if evidence=="bad_source"{"ftp://invalid.test/actual"}else{"https://example.test/actual"}]}]);
        host = host
            .with_response(
                "https://temper.test/tdata/Files('actuals')/$value",
                200,
                &actuals.to_string(),
            )
            .with_response(
                "https://temper.test/tdata/Forecasts?$filter=world_id eq 'world-1'&$top=513",
                200,
                &json!({"value":rows}).to_string(),
            );
        let mut ctx = context(
            "grade_hindcast",
            "Grade",
            json!({"world_id":"world-1","actuals_file_id":"actuals"}),
        );
        ctx.entity_type = "Hindcast".into();
        let result = invoke("grade_hindcast", ctx, host).await;
        assert_eq!(result.callback_action, "ScoreComplete", "{result:?}");
        assert_eq!(result.callback_params["graded_count"], "3", "{result:?}");
        assert_eq!(
            result.callback_params["learning_as_of"],
            if evidence == "eligible" {
                "2025-06-02T00:00:00Z"
            } else {
                ""
            }
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adoption_during_registration_keeps_the_batch_model_snapshot() {
    let frozen = json!({"version":"old-model","mode":"simulated","slope":1.0,"intercept":0.0,"evaluated_through":"2025-01-01T00:00:00Z","used_event_ids":[]});
    let newer = json!({"version":"new-model","mode":"simulated","slope":2.0,"intercept":0.0,"evaluated_through":"2025-02-01T00:00:00Z","used_event_ids":[]});
    let mut fields = world("simulated");
    fields["model_json"] = json!(newer.to_string());
    fields["adopted_learning_run_id"] = json!("new-run");
    fields["registration_model_json"] = json!(frozen.to_string());
    fields["forecast_learning_run_id"] = json!("old-run");
    fields["forecast_registered_at"] = json!("2026-09-15T00:00:00Z");
    fields["registration_nodes_json"] = json!(json!([{"Id":"node-1","Probability":"0.55","Provenance":"authored","ResolveBy":"2027-06-01T00:00:00Z","Statement":"Frozen future event"}]).to_string());
    let result = invoke(
        "register_forecasts",
        context("register_forecasts", "ForecastRegistered", fields),
        registration_host(),
    )
    .await;
    assert_eq!(result.callback_action, "ForecastPrepared", "{result:?}");
    assert_eq!(
        result.callback_params["forecast_model_version"],
        "old-model"
    );
    assert_eq!(
        result.callback_params["forecast_learning_run_id"],
        "old-run"
    );
    assert_eq!(result.callback_params["forecast_probability"], "0.55");
    assert_eq!(
        result.callback_params["forecast_question"],
        "Frozen future event"
    );
    assert_eq!(
        result.callback_params["forecast_registered_at"],
        "2026-09-15T00:00:00Z"
    );
    assert_eq!(result.callback_params["expected_registration_attempt"], 1);
    assert_eq!(
        serde_json::from_str::<Value>(
            result.callback_params["registration_model_json"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        frozen
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_hindcast_registers_at_explicit_clock_then_prepares_historical_learning() {
    let mut fields = world("observed");
    fields["hindcast_mode"] = json!("true");
    fields["last_ingest_date"] = json!("");
    let missing = invoke(
        "register_forecasts",
        context("register_forecasts", "PathsScored", fields.clone()),
        registration_host(),
    )
    .await;
    assert_eq!(
        missing.callback_action, "ForecastRegistrationFailed",
        "{missing:?}"
    );
    fields["last_ingest_date"] = json!("2025-03-01T00:00:00Z");
    let registration = invoke(
        "register_forecasts",
        context("register_forecasts", "RegisterForecasts", fields.clone()),
        registration_host(),
    )
    .await;
    assert_eq!(
        registration.callback_action, "ForecastPrepared",
        "{registration:?}"
    );
    assert_eq!(
        registration.callback_params["forecast_registered_at"],
        "2025-03-01T00:00:00Z"
    );
    assert_eq!(
        registration.callback_params["forecast_evidence_kind"],
        "historical"
    );
    assert_eq!(registration.callback_params["learning_mode"], "historical");
    for (key, value) in registration.callback_params.as_object().unwrap() {
        fields[key] = value.clone();
    }
    let forecast = json!({"status":"Scored","fields":{
        "event_node_id":"node-1","base_probability":"0.55","outcome":"yes",
        "registered_at":registration.callback_params["forecast_registered_at"],
        "resolved_at":"2025-06-01T00:00:00Z",
        "evidence_kind":registration.callback_params["forecast_evidence_kind"],
        "outcome_evidence_kind":"historical",
        "outcome_source_refs":"[\"https://example.test/recorded-actual\"]"
    }});
    let host = SimWasmHost::new()
        .with_default_response(503, "unexpected request")
        .with_response(
            "https://temper.test/tdata/Worlds('world-1')",
            200,
            &json!({"status":"Active","fields":fields}).to_string(),
        )
        .with_response(
            "https://temper.test/tdata/Forecasts?$filter=world_id eq 'world-1'&$top=513",
            200,
            &json!({"value":[forecast]}).to_string(),
        );
    let mut ctx = context(
        "learning_prepare",
        "Start",
        json!({"world_id":"world-1","mode":"historical","as_of":"2025-06-01T00:00:00Z","dataset_json":""}),
    );
    ctx.entity_type = "LearningRun".into();
    let prepared = invoke("learning_prepare", ctx, host).await;
    assert_eq!(prepared.callback_action, "Prepared", "{prepared:?}");
    let report: Value =
        serde_json::from_str(prepared.callback_params["prepared_json"].as_str().unwrap()).unwrap();
    assert_eq!(report["validation"].as_array().unwrap().len(), 1);
    assert_eq!(report["training"].as_array().unwrap().len(), 0);
}

struct SessionConfigureHost {
    inner: SimWasmHost,
    configure_requests: Arc<Mutex<Vec<Value>>>,
    http_requests: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl WasmHost for SessionConfigureHost {
    async fn http_call(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        self.http_requests
            .lock()
            .unwrap()
            .push((method.into(), url.into()));
        if method == "POST" && url.ends_with("/TemperPaw.Configure") {
            self.configure_requests
                .lock()
                .unwrap()
                .push(serde_json::from_str(body).expect("Session.Configure body must be JSON"));
        }
        self.inner.http_call(method, url, headers, body).await
    }

    async fn http_call_binary(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        self.inner
            .http_call_binary(method, url, headers, body)
            .await
    }

    fn get_secret(&self, key: &str) -> Result<String, String> {
        self.inner.get_secret(key)
    }

    fn log(&self, level: &str, message: &str) {
        self.inner.log(level, message);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn seed_world_records_research_session_from_the_create_response() {
    let host = SimWasmHost::new()
        .with_default_response(404, "unexpected request")
        .with_response(
            "https://temper.test/tdata/Workspaces",
            201,
            r#"{"entity_id":"workspace-1"}"#,
        )
        .with_response(
            "https://temper.test/tdata/Agents",
            201,
            r#"{"entity_id":"agent-1"}"#,
        )
        .with_response(
            "https://temper.test/tdata/Sessions",
            201,
            r#"{"entity_id":"session-1"}"#,
        )
        .with_response(
            "https://temper.test/tdata/Sessions('session-1')/TemperPaw.Configure",
            200,
            "{}",
        );
    let mut ctx = context(
        "seed_world",
        "Seed",
        json!({"agent_model":"fixture-model","agent_provider":"fixture-provider"}),
    );
    ctx.entity_state["status"] = json!("Seeding");
    ctx.entity_state["counters"] = json!({"research_attempt":7});
    let configure_requests = Arc::new(Mutex::new(Vec::new()));
    let host = SessionConfigureHost {
        http_requests: Arc::default(),
        inner: host,
        configure_requests: Arc::clone(&configure_requests),
    };
    let result = invoke("seed_world", ctx, host).await;
    let requests = configure_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "seed must configure its research session"
    );
    assert_eq!(
        requests[0]["tool_choice"], "required",
        "research must use explicit tool completion instead of a plain-text end turn"
    );
    assert!(result.success, "{result:?}");
    assert_eq!(
        result.callback_action, "ResearchSessionStarted",
        "{result:?}"
    );
    assert_eq!(result.callback_params["research_session_id"], "session-1");
    assert_ne!(result.callback_params["research_session_id"], "agent-1");
    assert_eq!(result.callback_params["expected_research_attempt"], 7);
}

fn corridor_session_host() -> SimWasmHost {
    let host = SimWasmHost::new().with_default_response(404, "unexpected request");
    let responses = [
        ("Workspaces", 201, json!({"entity_id":"workspace-1"})),
        ("Agents", 201, json!({"entity_id":"agent-1"})),
        ("Sessions", 201, json!({"entity_id":"session-1"})),
        ("Sessions('session-1')/TemperPaw.Configure", 200, json!({})),
        (
            "Worlds('world-1')",
            200,
            json!({
                "entity_id":"world-1",
                "fields":{"agent_model":"fixture-model","agent_provider":"fixture-provider"}
            }),
        ),
        (
            "Endpoints?$filter=world_id eq 'world-1'",
            200,
            json!({"value":[]}),
        ),
        ("Endpoints", 201, json!({"entity_id":"endpoint-1"})),
        ("Paths", 201, json!({"entity_id":"path-1"})),
        ("Paths('path-1')", 200, json!({})),
        ("Paths('subject-1')", 200, json!({})),
        ("Files('file-1')", 200, json!({"WorkspaceId":"workspace-1"})),
        (
            "Claims?$filter=world_id eq 'world-1'",
            200,
            json!({"value":[]}),
        ),
        (
            "EventNodes?$filter=world_id eq 'world-1'",
            200,
            json!({"value":[{
                "Id":"node-1","Status":"Confirmed","Provenance":"authored",
                "ResolveBy":"2026-09-01","Statement":"An overdue test event"
            }]}),
        ),
        (
            "Dwellers('dweller-1')",
            200,
            json!({"AgentId":"agent-1","Name":"Test dweller"}),
        ),
    ];
    responses
        .into_iter()
        .fold(host, |host, (path, status, body)| {
            host.with_response(
                &format!("https://temper.test/tdata/{path}"),
                status,
                &body.to_string(),
            )
        })
        .with_response(
            "https://temper.test/tdata/Files('file-1')/$value",
            200,
            "Fixture artifact content",
        )
}

#[tokio::test(flavor = "multi_thread")]
async fn corridor_workers_require_tools_in_outgoing_session_configuration() {
    // Exercise each production Configure builder, including both roles that
    // share animate_dwellers. A successful no-op cannot satisfy this test.
    let cases = [
        (
            "sample_endpoints",
            "SampleEndpoints",
            "World",
            "endpoint-writer",
            json!({"endpoint_budget":"1"}),
        ),
        (
            "sample_endpoints",
            "ResumeWriter",
            "Endpoint",
            "endpoint-writer",
            json!({"world_id":"world-1"}),
        ),
        (
            "spawn_repairers",
            "SubmitForBridge",
            "Claim",
            "repairer",
            json!({"world_id":"world-1","current_text":"A future claim"}),
        ),
        (
            "spawn_adversaries",
            "RepairComplete",
            "Path",
            "adversary",
            json!({"world_id":"world-1"}),
        ),
        (
            "decompose_endpoint",
            "SubmitForRepair",
            "Endpoint",
            "decomposer",
            json!({"world_id":"world-1","bundle_file_id":"file-1"}),
        ),
        (
            "consistency_gate",
            "SubmitForCheck",
            "Artifact",
            "checker",
            json!({"world_id":"world-1","content_file_id":"file-1"}),
        ),
        (
            "adjudicate_nodes",
            "AdjudicateNodes",
            "World",
            "adjudicator",
            json!({"last_adjudication_date":"2026-09-16"}),
        ),
        (
            "render_artifacts",
            "Render",
            "World",
            "renderer-author",
            json!({"canonical_path_id":"path-1"}),
        ),
        (
            "animate_dwellers",
            "AnimateDwellers",
            "World",
            "caster",
            json!({}),
        ),
        (
            "animate_dwellers",
            "SpawnNextDweller",
            "World",
            "dweller",
            json!({"dweller_ids":"[\"dweller-1\"]"}),
        ),
    ];
    let mut missing_tool_requirement = Vec::new();
    for (module, trigger, entity_type, role, mut fields) in cases {
        fields["agent_model"] = json!("fixture-model");
        fields["agent_provider"] = json!("fixture-provider");
        let mut ctx = context(module, trigger, fields);
        ctx.entity_type = entity_type.into();
        if entity_type != "World" {
            ctx.entity_id = "subject-1".into();
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let result = invoke(
            module,
            ctx,
            SessionConfigureHost {
                http_requests: Arc::default(),
                inner: corridor_session_host(),
                configure_requests: Arc::clone(&requests),
            },
        )
        .await;
        assert!(result.success, "{module}/{trigger}: {result:?}");
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "{module}/{trigger} must configure its worker"
        );
        let request = &requests[0];
        assert_eq!(request["agent_name"], role, "{module}/{trigger}");
        assert_eq!(request["model"], "fixture-model", "{module}/{trigger}");
        assert_eq!(
            request["provider"], "fixture-provider",
            "{module}/{trigger}"
        );
        assert_eq!(request["workspace_id"], "workspace-1", "{module}/{trigger}");
        assert!(
            request["tools_enabled"]
                .as_str()
                .unwrap()
                .split(',')
                .any(|tool| tool == "temper_action"),
            "{module}/{trigger} must be able to report its result through an action"
        );
        println!(
            "{module}/{trigger}: role={role} tool_choice={}",
            request["tool_choice"]
        );
        if request["tool_choice"] != "required" {
            missing_tool_requirement.push(format!("{module}/{trigger}"));
        }
    }
    assert!(
        missing_tool_requirement.is_empty(),
        "Tool-dependent workers must not finish with a plain-text end turn: {missing_tool_requirement:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn repairer_route_ids_attach_when_initial_field_is_missing_or_empty() {
    for prior in [None, Some(""), Some("[]"), Some("[\"earlier-route\"]")] {
        let mut fields = json!({"world_id":"world-1","current_text":"A future claim"});
        let mut expected = Vec::<String>::new();
        if let Some(prior) = prior {
            fields["path_ids"] = json!(prior);
            if !prior.is_empty() {
                expected = serde_json::from_str(prior).unwrap();
            }
        }
        fields["route_count"] = json!(expected.len().to_string());
        let mut ctx = context("spawn_repairers", "PrepareBridge", fields);
        ctx.entity_type = "Claim".into();
        ctx.entity_id = "claim-1".into();
        ctx.entity_state["status"] = json!("Bridging");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let result = invoke(
            "spawn_repairers",
            ctx,
            SessionConfigureHost {
                inner: corridor_session_host(),
                configure_requests: Arc::default(),
                http_requests: Arc::clone(&requests),
            },
        )
        .await;
        assert!(result.success, "initial path_ids {prior:?}: {result:?}");
        assert_eq!(result.callback_action, "RoutesAttached");
        expected.push("path-1".into());
        assert_eq!(
            result.callback_params["path_ids"],
            serde_json::to_string(&expected).unwrap()
        );
        assert_eq!(
            result.callback_params["route_count"],
            expected.len().to_string()
        );
        for collection in ["Paths", "Agents", "Sessions"] {
            assert_eq!(
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(method, url)| method == "POST"
                        && url == &format!("https://temper.test/tdata/{collection}"))
                    .count(),
                1,
                "exactly one {collection} create"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn repairer_route_ids_reject_malformed_values_before_any_http_effect() {
    for prior in ["broken", "null", "{}", "[7]"] {
        let mut ctx = context(
            "spawn_repairers",
            "PrepareBridge",
            json!({
                "world_id":"world-1", "current_text":"A future claim", "path_ids":prior,
            }),
        );
        ctx.entity_type = "Claim".into();
        ctx.entity_id = "claim-1".into();
        ctx.entity_state["status"] = json!("Bridging");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let result = invoke(
            "spawn_repairers",
            ctx,
            SessionConfigureHost {
                inner: corridor_session_host(),
                configure_requests: Arc::default(),
                http_requests: Arc::clone(&requests),
            },
        )
        .await;
        assert!(!result.success, "malformed path_ids {prior:?}: {result:?}");
        assert!(
            result
                .error
                .as_deref()
                .unwrap()
                .contains("invalid claim path ids")
        );
        assert!(
            requests.lock().unwrap().is_empty(),
            "invalid initial route data must not create a worker or route"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn registration_excludes_past_requirements_at_the_frozen_clock() {
    for (deadline, expected) in [
        ("2025-10-23", "ForecastRegistrationComplete"),
        ("2026-09-15", "ForecastRegistrationComplete"),
        ("2026-09-16T04:00:01Z", "ForecastRegistrationComplete"),
        ("2026-09-16T04:00:02Z", "ForecastPrepared"),
        ("2026-09-16", "ForecastPrepared"),
        ("2026-09-17", "ForecastPrepared"),
    ] {
        let host = registration_host().with_response(
            "https://temper.test/tdata/EventNodes?$filter=world_id eq 'world-1'&$top=513",
            200,
            &json!({"value":[{"Id":"node-1","AuthorAgentId":"dashboard","Status":"Proposed",
                "Probability":"0.55","Provenance":"authored","ResolveBy":deadline,
                "Statement":"Dated requirement"}]})
            .to_string(),
        );
        let result = invoke(
            "register_forecasts",
            context(
                "register_forecasts",
                "StartForecastRegistration",
                world("observed"),
            ),
            host,
        )
        .await;
        assert!(result.success, "{deadline}: {result:?}");
        assert_eq!(result.callback_action, expected, "{deadline}: {result:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn registration_cutoff_preserves_replay_and_uses_persisted_continuation_time() {
    for mode in ["observed", "historical", "simulated"] {
        for (deadline, expected) in [
            ("2025-01-01", "ForecastRegistrationComplete"),
            ("2025-01-02", "ForecastPrepared"),
        ] {
            let mut fields = world(mode);
            fields["forecast_registered_at"] = json!("2025-01-01T23:59:59Z");
            fields["last_ingest_date"] = json!("2000-01-01T00:00:00Z");
            let snapshot = invoke("register_forecasts", context("register_forecasts", "StartForecastRegistration", world(mode)), registration_host().with_response(
                "https://temper.test/tdata/EventNodes?$filter=world_id eq 'world-1'&$top=513", 200,
                &json!({"value":[{"Id":"node-1","AuthorAgentId":"dashboard","Status":"Proposed","Probability":"0.55","Provenance":"authored","ResolveBy":"2027-06-01","Statement":"Identity snapshot"}]}).to_string())).await;
            assert_eq!(snapshot.callback_action, "ForecastPrepared", "{snapshot:?}");
            fields["registration_model_json"] =
                snapshot.callback_params["registration_model_json"].clone();
            fields["registration_nodes_json"] = json!(json!([{"Id":"node-1","Probability":"0.55", "ResolveBy":deadline,"Statement":"Replay requirement"}]).to_string());
            let result = invoke(
                "register_forecasts",
                context("register_forecasts", "ForecastRegistered", fields),
                registration_host(),
            )
            .await;
            assert!(result.success, "{mode}/{deadline}: {result:?}");
            assert_eq!(
                result.callback_action, expected,
                "{mode}/{deadline}: {result:?}"
            );
            if expected == "ForecastPrepared" {
                assert_eq!(
                    result.callback_params["forecast_registered_at"],
                    "2025-01-01T23:59:59Z"
                );
            }
        }
    }
}
