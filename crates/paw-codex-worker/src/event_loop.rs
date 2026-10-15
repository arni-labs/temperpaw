async fn connect_and_watch_events(
    client: &reqwest::Client,
    config: &Config,
    url: &str,
) -> Result<()> {
    let response = client
        .get(url)
        .headers(event_stream_headers(config)?)
        .header(ACCEPT, "text/event-stream")
        .send()
        .await
        .with_context(|| format!("connect to event stream {url}"))?;

    if !response.status().is_success() {
        bail!("event stream returned {}", response.status());
    }

    info!(url, "connected to Temper event stream");

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            next = stream.next() => next,
            _ = sleep(event_stream_queue_poll_interval()), if config.poll_on_start => {
                debug!(url, "Temper event stream is open; polling queued Patrol work");
                claim_event_stream_backlog(client, config).await?;
                continue;
            }
        };

        let Some(chunk) = chunk else {
            return Ok(());
        };
        let chunk = chunk.context("read SSE chunk")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(frame_end) = buffer.find("\n\n") {
            let frame = buffer[..frame_end].to_string();
            buffer = buffer[frame_end + 2..].to_string();
            if let Some(data) = extract_sse_data(&frame) {
                handle_event_payload(client, config, &data).await?;
                if config.poll_on_start {
                    claim_event_stream_backlog(client, config).await?;
                }
            }
        }
    }
}

fn event_stream_queue_poll_interval() -> Duration {
    Duration::from_secs(15)
}

async fn claim_event_stream_backlog(client: &reqwest::Client, config: &Config) -> Result<()> {
    reconcile_dsf_model_sources(client, config).await?;
    claim_boot_queued_runs(client, config).await?;
    claim_boot_requested_review_runs(client, config).await?;
    claim_boot_queued_evaluation_runs(client, config).await?;
    claim_boot_queued_directed_evolution_work_items(client, config).await
}

async fn handle_event_payload(client: &reqwest::Client, config: &Config, data: &str) -> Result<()> {
    let event: EntityEvent = match serde_json::from_str(data) {
        Ok(event) => event,
        Err(error) => {
            debug!(%error, data, "ignoring non-entity SSE payload");
            return Ok(());
        }
    };

    match (event.entity_type.as_str(), event.status.as_str()) {
        ("WorkerRun", "Queued") => {
            handle_queued_worker_run(client, config, &event.entity_id).await?;
        }
        ("ReviewRun", "Requested") => {
            handle_requested_review_run(client, config, &event.entity_id).await?;
        }
        ("EvaluationRun", "Queued") => {
            handle_queued_evaluation_run(client, config, &event.entity_id).await?;
        }
        ("WorkItem", "Queued") => {
            handle_queued_directed_evolution_work_item(client, config, &event.entity_id).await?;
        }
        _ if event.entity_type.starts_with("Dsf") => {
            reconcile_dsf_model_sources(client, config).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_queued_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<()> {
    info!(worker_run_id, "saw queued WorkerRun");
    let worker_run = fetch_claimable_worker_run(client, config, worker_run_id).await?;
    if worker_run.status != "Queued" {
        debug!(worker_run_id, status = %worker_run.status, "WorkerRun no longer queued");
        return Ok(());
    }
    if !worker_run_is_claimable_by_local_codex(&worker_run, &config.worker_id) {
        debug!(
            worker_run_id,
            runner_kind = %worker_run.runner_kind,
            provider_id = %worker_run.provider_id,
            allowed_worker_id = %worker_run.allowed_worker_id,
            worker_id = %config.worker_id,
            "WorkerRun is not claimable by local Codex"
        );
        return Ok(());
    }

    claim_worker_run(client, config, worker_run_id).await?;
    start_local_worker_run(client, config, worker_run_id).await?;

    execute_worker_run(client, config, worker_run).await
}

async fn handle_running_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<()> {
    info!(
        worker_run_id,
        "saw running WorkerRun claimed by this worker"
    );
    let worker_run = fetch_worker_run(client, config, worker_run_id).await?;
    if !worker_run_is_recoverable_by_local_codex(&worker_run, &config.worker_id) {
        debug!(
            worker_run_id,
            status = %worker_run.status,
            runner_kind = %worker_run.runner_kind,
            allowed_worker_id = %worker_run.allowed_worker_id,
            worker_id = %worker_run.worker_id,
            local_worker_id = %config.worker_id,
            "WorkerRun is not recoverable by this local Codex worker"
        );
        return Ok(());
    }

    execute_worker_run(client, config, worker_run).await
}

async fn execute_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run: WorkerRunState,
) -> Result<()> {
    let worker_run_id = worker_run.id.clone();
    if let Some(patrol_id) = dsf_patrol_id(&worker_run.task) {
        return match run_dsf_model_patrol(client, config, &worker_run, &patrol_id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                post_action(
                    client,
                    config,
                    &worker_run.id,
                    "FailInvestigation",
                    json!({"error_message":format!("DSF investigation failed: {error}")}),
                )
                .await
            }
        };
    }
    let run_result = if let Some(snapshot_id) = extract_repo_sweep_snapshot_id(&worker_run.task) {
        run_repo_sweep(client, config, &worker_run, &snapshot_id).await
    } else if let Some(patrol_run_id) = extract_datadog_patrol_run_id(&worker_run.task) {
        run_datadog_patrol(client, config, &worker_run, &patrol_run_id).await
    } else if let Some(patrol_run_id) = extract_github_patrol_run_id(&worker_run.task) {
        run_github_patrol(client, config, &worker_run, &patrol_run_id).await
    } else if let Some(daily_brief_id) = extract_daily_brief_id(&worker_run.task) {
        run_daily_brief(client, config, &worker_run, &daily_brief_id).await
    } else {
        run_codex_for_worker(client, config, &worker_run).await
    };

    match run_result {
        Ok(summary) => report_done(client, config, &worker_run, &summary).await,
        Err(error) => {
            error!(worker_run_id = %worker_run_id, %error, "local Codex run failed");
            report_failed(client, config, &worker_run_id, &error.to_string()).await
        }
    }
}

async fn handle_requested_review_run(
    client: &reqwest::Client,
    config: &Config,
    review_run_id: &str,
) -> Result<()> {
    info!(review_run_id, "saw requested ReviewRun");
    let review_run = fetch_configured_review_run(client, config, review_run_id).await?;
    if review_run.status != "Requested" {
        debug!(review_run_id, status = %review_run.status, "ReviewRun no longer requested");
        return Ok(());
    }
    if review_run.worker_run_id.is_empty() {
        debug!(review_run_id, "ReviewRun is not configured yet");
        return Ok(());
    }

    let worker_run = fetch_worker_run(client, config, &review_run.worker_run_id).await?;
    if !config.enable_execution {
        debug!(
            review_run_id,
            worker_run_id = %worker_run.id,
            "leaving ReviewRun for an enabled reviewer agent"
        );
        return Ok(());
    }

    info!(
        review_run_id,
        action = REVIEW_CLAIM_LABEL,
        worker_run_id = %worker_run.id,
        "claiming ReviewRun for local Codex reviewer"
    );
    post_entity_action(
        client,
        config,
        "ReviewRuns",
        review_run_id,
        "Claim",
        json!({ "reviewer_id": config.worker_id }),
    )
    .await?;
    post_entity_action(
        client,
        config,
        "ReviewRuns",
        review_run_id,
        "StartReview",
        json!({}),
    )
    .await?;

    match run_codex_review(config, &worker_run, &review_run).await {
        Ok(decision) => {
            let body = json!({
                "review_summary": decision.summary,
                "live_e2e_summary": decision.live_e2e_summary,
                "verdict": decision.verdict,
            });
            let action = match decision.action {
                ReviewDecisionAction::Approve => "Approve",
                ReviewDecisionAction::RequestChanges => "RequestChanges",
                ReviewDecisionAction::Escalate => "Escalate",
            };
            post_entity_action(client, config, "ReviewRuns", review_run_id, action, body).await
        }
        Err(error) => {
            post_entity_action(
                client,
                config,
                "ReviewRuns",
                review_run_id,
                "Fail",
                json!({ "review_summary": format!("local Codex review failed: {error}") }),
            )
            .await
        }
    }
}

async fn handle_queued_evaluation_run(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
) -> Result<()> {
    info!(evaluation_run_id, "saw queued EvaluationRun");
    let evaluation_run = fetch_configured_evaluation_run(client, config, evaluation_run_id).await?;
    if evaluation_run.status != "Queued" {
        debug!(evaluation_run_id, status = %evaluation_run.status, "EvaluationRun no longer queued");
        return Ok(());
    }
    if evaluation_run.work_cycle_id.is_empty() {
        debug!(evaluation_run_id, "EvaluationRun is not configured yet");
        return Ok(());
    }

    let mut work_cycle = fetch_work_cycle(client, config, &evaluation_run.work_cycle_id).await?;
    if fail_terminal_queued_evaluation_if_needed(
        client,
        config,
        evaluation_run_id,
        &evaluation_run,
        &work_cycle,
        None,
    )
    .await?
    {
        return Ok(());
    }
    if work_cycle.implementer_worker_run_id.is_empty() {
        debug!(evaluation_run_id, work_cycle_id = %work_cycle.id, "WorkCycle has no implementer run yet");
        return Ok(());
    }
    let worker_run =
        fetch_worker_run(client, config, &work_cycle.implementer_worker_run_id).await?;
    let is_repo_sweep = worker_run_is_repo_sweep(&worker_run);

    if !work_cycle.review_passed && !work_cycle.reviewer_run_id.is_empty() {
        let review_run = fetch_review_run(client, config, &work_cycle.reviewer_run_id).await?;
        if fail_terminal_queued_evaluation_if_needed(
            client,
            config,
            evaluation_run_id,
            &evaluation_run,
            &work_cycle,
            Some(&review_run),
        )
        .await?
        {
            return Ok(());
        }
    }

    if !is_repo_sweep && !config.enable_execution {
        debug!(
            evaluation_run_id,
            worker_run_id = %worker_run.id,
            "leaving non-repo-sweep EvaluationRun for evaluator agent"
        );
        return Ok(());
    }

    if !work_cycle.review_passed && !work_cycle.reviewer_run_id.is_empty() {
        handle_requested_review_run(client, config, &work_cycle.reviewer_run_id).await?;
        work_cycle =
            fetch_work_cycle_until_review_passed(client, config, &evaluation_run.work_cycle_id)
                .await?;
        let review_run = if work_cycle.reviewer_run_id.is_empty() {
            None
        } else {
            Some(fetch_review_run(client, config, &work_cycle.reviewer_run_id).await?)
        };
        if fail_terminal_queued_evaluation_if_needed(
            client,
            config,
            evaluation_run_id,
            &evaluation_run,
            &work_cycle,
            review_run.as_ref(),
        )
        .await?
        {
            return Ok(());
        }
    }
    if !work_cycle.review_passed {
        debug!(
            evaluation_run_id,
            work_cycle_id = %work_cycle.id,
            "EvaluationRun is waiting for review_passed"
        );
        return Ok(());
    }

    if !evaluation_run.evaluator_id.is_empty() && evaluation_run.evaluator_id != config.worker_id {
        debug!(
            evaluation_run_id,
            evaluator_id = %evaluation_run.evaluator_id,
            worker_id = %config.worker_id,
            "EvaluationRun is reserved for a different evaluator"
        );
        return Ok(());
    }

    claim_evaluation_run(client, config, evaluation_run_id).await?;

    if !is_repo_sweep {
        return run_code_change_evaluation(client, config, evaluation_run_id, &worker_run).await;
    }

    info!(
        evaluation_run_id,
        action = EVALUATION_START_LABEL,
        "starting repo-sweep EvaluationRun"
    );
    post_entity_action(
        client,
        config,
        "EvaluationRuns",
        evaluation_run_id,
        "Start",
        json!({}),
    )
    .await?;

    let e2e_summary = repo_sweep_live_e2e_summary(client, config, &worker_run)
        .await
        .unwrap_or_else(|error| format!("Repo sweep live evidence lookup failed: {error}"));
    let results_json = serde_json::to_string(&json!({
        "kind": "repo_sweep_evaluation",
        "worker_run_id": worker_run.id,
        "work_cycle_id": work_cycle.id,
        "required_checks": evaluation_run.required_checks,
        "checks": [
            { "name": "worker_done", "status": worker_run.status },
            { "name": "repo_graph_snapshot_ready", "summary": e2e_summary }
        ]
    }))
    .context("serialize repo-sweep evaluation result")?;

    info!(
        evaluation_run_id,
        action = EVALUATION_PASS_LABEL,
        "passing repo-sweep EvaluationRun"
    );
    post_entity_action(
        client,
        config,
        "EvaluationRuns",
        evaluation_run_id,
        "Pass",
        json!({
            "results_json": results_json,
            "e2e_summary": e2e_summary
        }),
    )
    .await
}

async fn fail_terminal_queued_evaluation_if_needed(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
    evaluation_run: &EvaluationRunState,
    work_cycle: &WorkCycleState,
    review_run: Option<&ReviewRunState>,
) -> Result<bool> {
    let Some(blocker) = queued_evaluation_terminal_blocker(work_cycle, review_run) else {
        return Ok(false);
    };

    if evaluation_run_is_claimable_by_worker(evaluation_run, &config.worker_id) {
        fail_queued_evaluation_run(client, config, evaluation_run_id, &blocker).await?;
    } else {
        warn!(
            evaluation_run_id,
            evaluator_id = %evaluation_run.evaluator_id,
            worker_id = %config.worker_id,
            failure_classification = %blocker.failure_classification,
            "terminal queued EvaluationRun cannot be failed by this worker because it is reserved for a different evaluator"
        );
    }
    Ok(true)
}
