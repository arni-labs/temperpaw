#[test]
fn dsf_fingerprint_ignores_collection_noise_but_preserves_drift_and_job_identity() {
    let source = json!({"intended_configuration":"{\"replicas\":2}","intended_revision":"abc"});
    let observed = json!({"subject_type":"DsfRailwayServiceInstance","subject_id":"api","source":"railway","status":"Measured","outcome":"SUCCESS","observed_revision":"abc","summary":"{\"replicas\":2,\"createdAt\":\"old\",\"jobs\":[{\"id\":\"job-1\",\"age_seconds\":5}]}"});
    let first = dsf_investigation_key(&source, &observed).unwrap();
    let mut activity = observed.clone();
    activity["summary"] = json!("{\"last_active_at\":\"2026-09-06T01:00:00Z\"}");
    let activity_key = dsf_investigation_key(&source, &activity).unwrap();
    activity["summary"] = json!("{\"last_active_at\":\"2026-09-06T02:00:00Z\"}");
    assert_ne!(
        activity_key,
        dsf_investigation_key(&source, &activity).unwrap(),
        "actual activity is evidence, not collection noise"
    );
    let mut unchanged = observed.clone();
    unchanged["observed_at_ms"] = json!(200);
    unchanged["source_event_id"] = json!("new");
    unchanged["summary"] = json!(
        "{\"replicas\":2,\"createdAt\":\"new\",\"jobs\":[{\"id\":\"job-1\",\"age_seconds\":9}]}"
    );
    assert_eq!(first, dsf_investigation_key(&source, &unchanged).unwrap());
    unchanged["observed_revision"] = json!("different");
    assert_ne!(first, dsf_investigation_key(&source, &unchanged).unwrap());
    let mut desired = source.clone();
    desired["intended_configuration"] = json!("{\"replicas\":3}");
    assert_ne!(first, dsf_investigation_key(&desired, &observed).unwrap());
    let mut changed = observed.clone();
    changed["summary"] = json!("{\"replicas\":2,\"jobs\":[{\"id\":\"job-2\"}]}");
    assert_ne!(first, dsf_investigation_key(&source, &changed).unwrap());
}
#[test]
fn dsf_invocation_uses_subscription_auth_and_only_its_temper_mcp_binding() {
    let args = dsf_codex_args(
        Path::new("/tmp/work"),
        "/opt/bin/temper-mcp",
        "https://temper.test",
        "investigate",
    )
    .unwrap();
    let args = args
        .iter()
        .map(|a| a.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert!(args.contains(&"forced_login_method=\"chatgpt\"".into()));
    assert!(args.contains(&"mcp_servers.temper.env_vars=[\"TEMPER_API_KEY\"]".into()));
    assert!(args.contains(&"--ignore-user-config".into()));
    assert!(args.contains(&"mcp_servers.temper.command=\"/opt/bin/temper-mcp\"".into()));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("bearer_token") || a.contains(".url="))
    );
    assert!(
        dsf_codex_args(
            Path::new("/tmp/work"),
            "/opt/bin/temper-mcp",
            "https://user:secret@temper.test",
            "x"
        )
        .is_err()
    );
}

// Scripted HTTP responses test the real worker client, not a second scheduler.
async fn dsf_http_fixture(
    responses: Vec<(u16, Value)>,
) -> (String, tokio::task::JoinHandle<Vec<RecordedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_test_http_request(&mut stream).await.unwrap());
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 {status} Fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn dsf_queue_recovers_created_record_and_deduplicates_unchanged_evidence() {
    let source = json!({"id":"api","intended_configuration":"{}","intended_revision":"abc"});
    let observation = json!({"subject_type":"DsfRailwayServiceInstance","subject_id":"api","source":"railway","status":"Measured","summary":"{\"replicas\":2}"});
    let key = dsf_investigation_key(&source, &observation).unwrap();
    let (url, server) = dsf_http_fixture(vec![
        (404, json!({})),
        (201, json!({})),
        (200, json!({})),
        (200, json!({"id":key,"status":"Complete"})),
        (200, json!({"id":key,"status":"Created"})),
        (200, json!({})),
    ])
    .await;
    let config = terminal_evaluation_config(url);
    let client = reqwest::Client::new();
    queue_dsf_investigation(&client, &config, &source, "obs-1", &observation)
        .await
        .unwrap();
    queue_dsf_investigation(&client, &config, &source, "obs-2", &observation)
        .await
        .unwrap();
    queue_dsf_investigation(&client, &config, &source, "obs-3", &observation)
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 6);
    let first: Value = serde_json::from_str(&requests[2].body).unwrap();
    let recovered: Value = serde_json::from_str(&requests[5].body).unwrap();
    assert_eq!(first["worker_run_id"], recovered["worker_run_id"]);
    assert_eq!(first["investigation_key"], key);
    assert_eq!(recovered["observation_id"], "obs-3");
    assert!(
        requests[3].body.is_empty(),
        "completed unchanged evidence performs only a GET"
    );
}

#[tokio::test]
async fn dsf_queue_recovers_missing_worker_and_lost_terminal_ack_without_new_agent() {
    let source = json!({"intended_configuration":"{}"});
    let obs = json!({"subject_type":"DsfFlow","subject_id":"story","source":"code","status":"Measured","summary":"{}"});
    let key = dsf_investigation_key(&source, &obs).unwrap();
    let (url, server) = dsf_http_fixture(vec![
        (
            200,
            json!({"id":key,"status":"Queued","worker_run_id":"worker-one"}),
        ),
        (404, json!({})),
        (200, json!({})),
        (
            200,
            json!({"id":key,"status":"Running","worker_run_id":"worker-one"}),
        ),
        (200, json!({"status":"Done"})),
        (200, json!({})),
    ])
    .await;
    let config = terminal_evaluation_config(url);
    let client = reqwest::Client::new();
    for _ in 0..2 {
        queue_dsf_investigation(&client, &config, &source, "obs", &obs)
            .await
            .unwrap();
    }
    let requests = server.await.unwrap();
    assert!(
        requests[2]
            .path
            .ends_with("TemperPaw.Patrol.ReconcileModelWorker")
    );
    assert!(
        requests[5]
            .path
            .ends_with("TemperPaw.Patrol.ReplayInvestigationResult")
    );
    assert_eq!(requests[2].body, "{}");
    assert_eq!(requests[5].body, "{}");
}

#[tokio::test]
async fn dsf_result_requires_actual_model_provenance_and_linked_ordinary_work() {
    let result = DsfModelResult {
        disposition: "maintenance".into(),
        summary: "updated".into(),
        model_refs: vec![DsfModelReference {
            entity_type: "DsfFlow".into(),
            id: "flow".into(),
        }],
        intent_id: String::new(),
        effort_id: String::new(),
        ask_ids: vec![],
    };
    let (url, server) = dsf_http_fixture(vec![
        (200, json!({"provenance_ref":"other"})),
        (200, json!({"provenance_ref":"DsfObservations('obs')"})),
    ])
    .await;
    let config = terminal_evaluation_config(url);
    let client = reqwest::Client::new();
    assert!(
        dsf_verify_result(&client, &config, "obs", &result)
            .await
            .is_err()
    );
    dsf_verify_result(&client, &config, "obs", &result)
        .await
        .unwrap();
    server.await.unwrap();
    let result = DsfModelResult {
        disposition: "follow_up".into(),
        summary: "investigated".into(),
        model_refs: vec![],
        intent_id: "intent".into(),
        effort_id: "effort".into(),
        ask_ids: vec!["ask".into()],
    };
    let (url, server) = dsf_http_fixture(vec![
        (200, json!({"request_text":"repair from obs"})),
        (200, json!({"intent_id":"intent"})),
        (200, json!({"effort_id":"different"})),
    ])
    .await;
    assert!(
        dsf_verify_result(&client, &terminal_evaluation_config(url), "obs", &result)
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn dsf_child_receives_only_factory_credential_and_forced_subscription_arguments() {
    let _guard = ENV_LOCK.lock().await;
    let _factory = EnvOverride::set("DSF_FACTORY_AGENT_TOKEN", OsString::from("fixture-factory"));
    let _worker = EnvOverride::set("WORKER_TOKEN", OsString::from("fixture-daemon"));
    let _api = EnvOverride::set("OPENAI_API_KEY", OsString::from("fixture-api"));
    let root = unique_temp_dir();
    fs::create_dir_all(&root).unwrap();
    let script = root.join("inspect-child.sh");
    fs::write(&script,"#!/bin/sh\n[ \"$TEMPER_API_KEY\" = fixture-factory ] || exit 31\n[ -z \"$WORKER_TOKEN$DSF_FACTORY_AGENT_TOKEN$OPENAI_API_KEY\" ] || exit 32\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = terminal_evaluation_config("https://temper.test".into());
    config.codex_bin = script.to_string_lossy().into();
    let args = dsf_codex_args(
        &root,
        "/opt/bin/temper-mcp",
        &config.temper_url,
        "investigate",
    )
    .unwrap();
    let output = run_codex_exec_command_with_args(&config, &root, args, "fixture child")
        .await
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("forced_login_method=\"chatgpt\""));
    assert!(text.contains("mcp_servers.temper.env_vars=[\"TEMPER_API_KEY\"]"));
    assert!(!text.contains("fixture-factory"));
    fs::remove_dir_all(root).unwrap();
}
