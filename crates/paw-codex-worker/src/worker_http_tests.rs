#[test]
fn worker_headers_identify_the_daemon_as_an_agent_principal() {
    let config = Config {
        temper_url: "http://127.0.0.1:3497".to_string(),
        tenant: "default".to_string(),
        worker_id: "mac-mini-codex-1".to_string(),
        worker_token: Some("secret".to_string()),
        workspace_root: PathBuf::from("/tmp/worktrees"),
        repo_root: PathBuf::from("/tmp/temperpaw"),
        codex_bin: "codex".to_string(),
        max_concurrent_runs: 1,
        enable_execution: false,
        poll_on_start: true,
        codex_exec_smoke: false,
        codex_exec_timeout: Duration::from_secs(30),
    };

    let headers = headers(&config).expect("headers");

    assert_eq!(
        headers
            .get("x-temper-principal-kind")
            .and_then(|value| value.to_str().ok()),
        Some("agent")
    );
    assert_eq!(
        headers
            .get("x-temper-principal-id")
            .and_then(|value| value.to_str().ok()),
        Some("mac-mini-codex-1")
    );
}

#[test]
fn event_stream_headers_use_token_without_worker_principal_headers() {
    let config = Config {
        temper_url: "http://127.0.0.1:3497".to_string(),
        tenant: "default".to_string(),
        worker_id: "mac-mini-codex-1".to_string(),
        worker_token: Some("secret".to_string()),
        workspace_root: PathBuf::from("/tmp/worktrees"),
        repo_root: PathBuf::from("/tmp/temperpaw"),
        codex_bin: "codex".to_string(),
        max_concurrent_runs: 1,
        enable_execution: false,
        poll_on_start: true,
        codex_exec_smoke: false,
        codex_exec_timeout: Duration::from_secs(30),
    };

    let headers = event_stream_headers(&config).expect("headers");

    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer secret")
    );
    assert!(
        !headers.contains_key("x-temper-principal-kind"),
        "current Temper event stream rejects agent principals; WorkerRun actions still use worker identity"
    );
}

#[test]
fn event_urls_try_planned_and_current_temper_streams() {
    let config = Config {
        temper_url: "http://127.0.0.1:3497".to_string(),
        tenant: "default".to_string(),
        worker_id: "mac-mini-codex-1".to_string(),
        worker_token: Some("secret".to_string()),
        workspace_root: PathBuf::from("/tmp/worktrees"),
        repo_root: PathBuf::from("/tmp/temperpaw"),
        codex_bin: "codex".to_string(),
        max_concurrent_runs: 1,
        enable_execution: false,
        poll_on_start: true,
        codex_exec_smoke: false,
        codex_exec_timeout: Duration::from_secs(30),
    };

    assert_eq!(
        config.events_urls(),
        vec![
            "http://127.0.0.1:3497/tdata/$events".to_string(),
            "http://127.0.0.1:3497/observe/events/stream".to_string()
        ]
    );
}

#[test]
fn worker_event_status_reads_observe_stream_shape() {
    let event: EntityEvent = serde_json::from_value(json!({
        "seq": 12,
        "entity_type": "WorkerRun",
        "entity_id": "wr-queued",
        "action": "Create",
        "status": "Queued",
        "tenant": "default"
    }))
    .expect("event should parse");

    assert_eq!(event.entity_type, "WorkerRun");
    assert_eq!(event.entity_id, "wr-queued");
    assert_eq!(event.status, "Queued");
}

#[test]
fn worker_event_status_accepts_status_aliases() {
    let event: EntityEvent = serde_json::from_value(json!({
        "entityType": "WorkerRun",
        "entityId": "wr-queued",
        "new_status": "Queued"
    }))
    .expect("event should parse");

    assert_eq!(event.entity_type, "WorkerRun");
    assert_eq!(event.entity_id, "wr-queued");
    assert_eq!(event.status, "Queued");
}
