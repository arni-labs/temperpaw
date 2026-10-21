async fn fetch_claimable_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<WorkerRunState> {
    let mut worker_run = fetch_worker_run(client, config, worker_run_id).await?;
    for _ in 0..10 {
        if worker_run.status != "Queued"
            || (!worker_run.runner_kind.is_empty() && !worker_run.allowed_worker_id.is_empty())
        {
            return Ok(worker_run);
        }
        sleep(Duration::from_millis(200)).await;
        worker_run = fetch_worker_run(client, config, worker_run_id).await?;
    }
    Ok(worker_run)
}

async fn fetch_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<WorkerRunState> {
    let response = client
        .get(config.entity_url("WorkerRuns", worker_run_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch WorkerRun")?;
    if !response.status().is_success() {
        bail!("fetch WorkerRun returned {}", response.status());
    }
    let body = response.json().await.context("parse WorkerRun")?;
    worker_run_from_odata_value(body)
}

async fn fetch_configured_review_run(
    client: &reqwest::Client,
    config: &Config,
    review_run_id: &str,
) -> Result<ReviewRunState> {
    let mut review_run = fetch_review_run(client, config, review_run_id).await?;
    for _ in 0..10 {
        if review_run.status != "Requested" || !review_run.worker_run_id.is_empty() {
            return Ok(review_run);
        }
        sleep(Duration::from_millis(200)).await;
        review_run = fetch_review_run(client, config, review_run_id).await?;
    }
    Ok(review_run)
}

async fn fetch_review_run(
    client: &reqwest::Client,
    config: &Config,
    review_run_id: &str,
) -> Result<ReviewRunState> {
    let response = client
        .get(config.entity_url("ReviewRuns", review_run_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch ReviewRun")?;
    if !response.status().is_success() {
        bail!("fetch ReviewRun returned {}", response.status());
    }
    let body = response.json().await.context("parse ReviewRun")?;
    review_run_from_odata_value(body)
}

async fn fetch_configured_evaluation_run(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
) -> Result<EvaluationRunState> {
    let mut evaluation_run = fetch_evaluation_run(client, config, evaluation_run_id).await?;
    for _ in 0..10 {
        if evaluation_run.status != "Queued" || !evaluation_run.work_cycle_id.is_empty() {
            return Ok(evaluation_run);
        }
        sleep(Duration::from_millis(200)).await;
        evaluation_run = fetch_evaluation_run(client, config, evaluation_run_id).await?;
    }
    Ok(evaluation_run)
}

async fn fetch_evaluation_run(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
) -> Result<EvaluationRunState> {
    let response = client
        .get(config.entity_url("EvaluationRuns", evaluation_run_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch EvaluationRun")?;
    if !response.status().is_success() {
        bail!("fetch EvaluationRun returned {}", response.status());
    }
    let body = response.json().await.context("parse EvaluationRun")?;
    evaluation_run_from_odata_value(body)
}

async fn fetch_work_cycle(
    client: &reqwest::Client,
    config: &Config,
    work_cycle_id: &str,
) -> Result<WorkCycleState> {
    let response = client
        .get(config.entity_url("WorkCycles", work_cycle_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch WorkCycle")?;
    if !response.status().is_success() {
        bail!("fetch WorkCycle returned {}", response.status());
    }
    let body = response.json().await.context("parse WorkCycle")?;
    work_cycle_from_odata_value(body)
}

async fn fetch_work_cycle_until_review_passed(
    client: &reqwest::Client,
    config: &Config,
    work_cycle_id: &str,
) -> Result<WorkCycleState> {
    let mut work_cycle = fetch_work_cycle(client, config, work_cycle_id).await?;
    for _ in 0..20 {
        if work_cycle.review_passed || work_cycle_status_is_terminal(&work_cycle.status) {
            return Ok(work_cycle);
        }
        sleep(Duration::from_millis(250)).await;
        work_cycle = fetch_work_cycle(client, config, work_cycle_id).await?;
    }
    Ok(work_cycle)
}

async fn claim_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<()> {
    info!(
        worker_run_id,
        action = ACTION_CLAIM_LABEL,
        "claiming WorkerRun"
    );
    post_action(
        client,
        config,
        worker_run_id,
        "Claim",
        json!({ "worker_id": config.worker_id }),
    )
    .await
}

async fn report_worker_heartbeat(client: &reqwest::Client, config: &Config) -> Result<()> {
    post_entity_action(
        client,
        config,
        "WorkerAgents",
        &config.worker_id,
        "ReportHeartbeat",
        json!({
            "last_seen_at": generated_at_label(),
            "status_summary": "paw-codex-worker running under openclaw launchd",
            "capabilities": worker_capabilities().join(","),
        }),
    )
    .await
}

async fn start_local_worker_run(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
) -> Result<()> {
    post_action(client, config, worker_run_id, "StartLocal", json!({})).await
}

async fn report_done(
    client: &reqwest::Client,
    config: &Config,
    worker_run: &WorkerRunState,
    summary: &str,
) -> Result<()> {
    info!(
        worker_run_id = %worker_run.id,
        action = ACTION_REPORT_DONE_LABEL,
        "reporting WorkerRun done"
    );
    post_action(
        client,
        config,
        &worker_run.id,
        "ReportDone",
        json!({
            "result_summary": summary,
            "proof_packet_id": "",
            "branch_name": worker_run.branch_name,
        }),
    )
    .await
}

async fn report_failed(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
    error_message: &str,
) -> Result<()> {
    info!(
        worker_run_id,
        action = ACTION_REPORT_FAILED_LABEL,
        "reporting WorkerRun failed"
    );
    post_action(
        client,
        config,
        worker_run_id,
        "ReportFailed",
        json!({ "error_message": error_message }),
    )
    .await
}

async fn claim_evaluation_run(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
) -> Result<()> {
    info!(
        evaluation_run_id,
        action = EVALUATION_CLAIM_LABEL,
        "claiming EvaluationRun"
    );
    post_entity_action(
        client,
        config,
        "EvaluationRuns",
        evaluation_run_id,
        "Claim",
        json!({ "evaluator_id": config.worker_id }),
    )
    .await
}

async fn fail_queued_evaluation_run(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
    blocker: &EvaluationTerminalBlocker,
) -> Result<()> {
    claim_evaluation_run(client, config, evaluation_run_id).await?;
    info!(
        evaluation_run_id,
        action = EVALUATION_FAIL_LABEL,
        failure_classification = %blocker.failure_classification,
        "failing queued EvaluationRun that cannot reach review_passed"
    );
    post_entity_action(
        client,
        config,
        "EvaluationRuns",
        evaluation_run_id,
        "Fail",
        json!({
            "results_json": blocker.results_json,
            "error_message": blocker.error_message,
            "failure_classification": blocker.failure_classification,
        }),
    )
    .await
}

async fn post_action(
    client: &reqwest::Client,
    config: &Config,
    worker_run_id: &str,
    action: &str,
    body: Value,
) -> Result<()> {
    post_entity_action(client, config, "WorkerRuns", worker_run_id, action, body).await
}

async fn post_entity_action(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    entity_id: &str,
    action: &str,
    body: Value,
) -> Result<()> {
    post_entity_action_with_namespace(
        client,
        config,
        entity_set,
        entity_id,
        "TemperPaw.Patrol",
        action,
        body,
    )
    .await
}

async fn post_entity_action_with_namespace(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    entity_id: &str,
    namespace: &str,
    action: &str,
    body: Value,
) -> Result<()> {
    post_entity_action_with_namespace_observed(
        client, config, entity_set, entity_id, namespace, action, body, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn post_entity_action_with_namespace_observed(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    entity_id: &str,
    namespace: &str,
    action: &str,
    body: Value,
    observe_metadata: Option<&str>,
) -> Result<()> {
    let response = client
        .post(config.entity_action_url_with_namespace(
            entity_set, entity_id, namespace, action,
        ))
        .headers(headers_with_observe_metadata(config, observe_metadata)?)
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("dispatch {entity_set}.{action}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("{entity_set}.{action} returned {status}: {text}");
    }
    Ok(())
}

async fn create_entity_with_observe_metadata(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    body: Value,
    observe_metadata: Option<&str>,
) -> Result<String> {
    let response = client
        .post(format!("{}/tdata/{}", config.temper_url, entity_set))
        .headers(headers_with_observe_metadata(config, observe_metadata)?)
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("create {entity_set}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("create {entity_set} returned {status}: {text}");
    }
    let value: Value = response.json().await.context("parse create entity response")?;
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("create {entity_set} response was missing entity_id");
    }
    Ok(id)
}

fn worker_run_from_odata_value(value: Value) -> Result<WorkerRunState> {
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("WorkerRun response was missing entity_id");
    }

    Ok(WorkerRunState {
        id,
        status: first_string(
            &value,
            &fields,
            &["status", "Status"],
            &["status", "Status"],
        ),
        task: first_string(&value, &fields, &["task", "Task"], &["task", "Task"]),
        work_cycle_id: first_string(
            &value,
            &fields,
            &["work_cycle_id", "WorkCycleId"],
            &["work_cycle_id", "WorkCycleId"],
        ),
        worktree_path: first_string(
            &value,
            &fields,
            &["worktree_path", "WorktreePath"],
            &["worktree_path", "WorktreePath"],
        ),
        branch_name: first_string(
            &value,
            &fields,
            &["branch_name", "BranchName"],
            &["branch_name", "BranchName"],
        ),
        runner_kind: first_string(
            &value,
            &fields,
            &["runner_kind", "RunnerKind"],
            &["runner_kind", "RunnerKind"],
        ),
        allowed_worker_id: first_string(
            &value,
            &fields,
            &["allowed_worker_id", "AllowedWorkerId"],
            &["allowed_worker_id", "AllowedWorkerId"],
        ),
        worker_id: first_string(
            &value,
            &fields,
            &["worker_id", "WorkerId"],
            &["worker_id", "WorkerId"],
        ),
        provider_id: first_string(
            &value,
            &fields,
            &["provider_id", "ProviderId"],
            &["provider_id", "ProviderId"],
        ),
        required_capabilities: first_string(
            &value,
            &fields,
            &["required_capabilities", "RequiredCapabilities"],
            &["required_capabilities", "RequiredCapabilities"],
        ),
    })
}

fn review_run_from_odata_value(value: Value) -> Result<ReviewRunState> {
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("ReviewRun response was missing entity_id");
    }

    Ok(ReviewRunState {
        status: first_string(
            &value,
            &fields,
            &["status", "Status"],
            &["status", "Status"],
        ),
        worker_run_id: first_string(
            &value,
            &fields,
            &["worker_run_id", "WorkerRunId"],
            &["worker_run_id", "WorkerRunId"],
        ),
        proof_packet_id: first_string(
            &value,
            &fields,
            &["proof_packet_id", "ProofPacketId"],
            &["proof_packet_id", "ProofPacketId"],
        ),
    })
}

fn evaluation_run_from_odata_value(value: Value) -> Result<EvaluationRunState> {
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("EvaluationRun response was missing entity_id");
    }

    Ok(EvaluationRunState {
        status: first_string(
            &value,
            &fields,
            &["status", "Status"],
            &["status", "Status"],
        ),
        work_cycle_id: first_string(
            &value,
            &fields,
            &["work_cycle_id", "WorkCycleId"],
            &["work_cycle_id", "WorkCycleId"],
        ),
        evaluator_id: first_string(
            &value,
            &fields,
            &["evaluator_id", "EvaluatorId"],
            &["evaluator_id", "EvaluatorId"],
        ),
        required_checks: first_string(
            &value,
            &fields,
            &["required_checks", "RequiredChecks"],
            &["required_checks", "RequiredChecks"],
        ),
    })
}

async fn fetch_directed_evolution_work_item(
    client: &reqwest::Client,
    config: &Config,
    work_item_id: &str,
) -> Result<DirectedEvolutionWorkItemState> {
    let response = client
        .get(config.entity_url("WorkItems", work_item_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch Directed Evolution WorkItem")?;
    if !response.status().is_success() {
        bail!("fetch WorkItem returned {}", response.status());
    }
    let body = response.json().await.context("parse WorkItem")?;
    directed_evolution_work_item_from_odata_value(body)
}

fn directed_evolution_work_item_from_odata_value(
    value: Value,
) -> Result<DirectedEvolutionWorkItemState> {
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("WorkItem response was missing entity_id");
    }

    Ok(DirectedEvolutionWorkItemState {
        id,
        status: first_string(
            &value,
            &fields,
            &["status", "Status"],
            &["status", "Status"],
        ),
        role: first_string(&value, &fields, &["role", "Role"], &["role", "Role"]),
        target_entity_type: first_string(
            &value,
            &fields,
            &["target_entity_type", "TargetEntityType"],
            &["target_entity_type", "TargetEntityType"],
        ),
        target_entity_id: first_string(
            &value,
            &fields,
            &["target_entity_id", "TargetEntityId"],
            &["target_entity_id", "TargetEntityId"],
        ),
        prompt_ref: first_string(
            &value,
            &fields,
            &["prompt_ref", "PromptRef"],
            &["prompt_ref", "PromptRef"],
        ),
        context_ref: first_string(
            &value,
            &fields,
            &["context_ref", "ContextRef"],
            &["context_ref", "ContextRef"],
        ),
        output_schema_ref: first_string(
            &value,
            &fields,
            &["output_schema_ref", "OutputSchemaRef"],
            &["output_schema_ref", "OutputSchemaRef"],
        ),
        correlation_json: first_string(
            &value,
            &fields,
            &["correlation_json", "CorrelationJson"],
            &["correlation_json", "CorrelationJson"],
        ),
        worker_run_id: first_string(
            &value,
            &fields,
            &["worker_run_id", "WorkerRunId"],
            &["worker_run_id", "WorkerRunId"],
        ),
    })
}

fn work_cycle_from_odata_value(value: Value) -> Result<WorkCycleState> {
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let id = first_string(&value, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
    if id.is_empty() {
        bail!("WorkCycle response was missing entity_id");
    }

    Ok(WorkCycleState {
        id,
        status: first_string(
            &value,
            &fields,
            &["status", "Status"],
            &["status", "Status"],
        ),
        implementer_worker_run_id: first_string(
            &value,
            &fields,
            &["implementer_worker_run_id", "ImplementerWorkerRunId"],
            &["implementer_worker_run_id", "ImplementerWorkerRunId"],
        ),
        reviewer_run_id: first_string(
            &value,
            &fields,
            &["reviewer_run_id", "ReviewerRunId"],
            &["reviewer_run_id", "ReviewerRunId"],
        ),
        review_passed: first_bool(
            &value,
            &fields,
            &["review_passed", "ReviewPassed"],
            &["review_passed", "ReviewPassed"],
        ),
    })
}

fn queued_evaluation_terminal_blocker(
    work_cycle: &WorkCycleState,
    review_run: Option<&ReviewRunState>,
) -> Option<EvaluationTerminalBlocker> {
    if work_cycle_status_is_terminal(&work_cycle.status) {
        return Some(evaluation_terminal_blocker(
            work_cycle,
            review_run,
            "parent_work_cycle_terminal",
            format!(
                "Queued EvaluationRun cannot proceed because WorkCycle {} is {}.",
                work_cycle.id, work_cycle.status
            ),
        ));
    }

    if work_cycle.review_passed {
        return None;
    }

    let review_run = review_run?;
    if review_status_precludes_approval(&review_run.status) {
        return Some(evaluation_terminal_blocker(
            work_cycle,
            Some(review_run),
            "review_terminal_without_approval",
            format!(
                "Queued EvaluationRun cannot proceed because ReviewRun {} is {} and WorkCycle {} has review_passed=false.",
                non_empty_or(&work_cycle.reviewer_run_id, "unknown"),
                review_run.status,
                work_cycle.id
            ),
        ));
    }

    None
}

fn evaluation_terminal_blocker(
    work_cycle: &WorkCycleState,
    review_run: Option<&ReviewRunState>,
    failure_classification: &str,
    error_message: String,
) -> EvaluationTerminalBlocker {
    let review_status = review_run
        .map(|review| review.status.as_str())
        .unwrap_or("not_fetched");
    let results_json = serde_json::to_string(&json!({
        "kind": "queued_evaluation_terminalized",
        "work_cycle_id": work_cycle.id,
        "work_cycle_status": work_cycle.status,
        "reviewer_run_id": work_cycle.reviewer_run_id,
        "review_run_status": review_status,
        "review_passed": work_cycle.review_passed,
        "failure_classification": failure_classification
    }))
    .unwrap_or_else(|_| "{}".to_string());

    EvaluationTerminalBlocker {
        results_json,
        error_message,
        failure_classification: failure_classification.to_string(),
    }
}

fn work_cycle_status_is_terminal(status: &str) -> bool {
    matches!(status, "Complete" | "Failed")
}

fn review_status_precludes_approval(status: &str) -> bool {
    matches!(status, "ChangesRequested" | "Escalated" | "Failed")
}

fn evaluation_run_is_claimable_by_worker(
    evaluation_run: &EvaluationRunState,
    worker_id: &str,
) -> bool {
    evaluation_run.evaluator_id.trim().is_empty() || evaluation_run.evaluator_id == worker_id
}

fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn worker_run_is_claimable_by_local_codex(worker_run: &WorkerRunState, worker_id: &str) -> bool {
    worker_run.status == "Queued"
        && worker_run.runner_kind == "local_codex"
        && worker_run.allowed_worker_id == worker_id
        && worker_run_has_worktree_assignment(worker_run)
        && worker_run_required_capabilities_satisfied(worker_run)
}

fn worker_run_is_recoverable_by_local_codex(worker_run: &WorkerRunState, worker_id: &str) -> bool {
    worker_run.status == "Running"
        && worker_run.runner_kind == "local_codex"
        && worker_run.allowed_worker_id == worker_id
        && worker_run.worker_id == worker_id
        && worker_run_has_worktree_assignment(worker_run)
        && worker_run_required_capabilities_satisfied(worker_run)
}

fn worker_run_has_worktree_assignment(worker_run: &WorkerRunState) -> bool {
    !worker_run.worktree_path.trim().is_empty() || !worker_run.branch_name.trim().is_empty()
}

fn worker_run_required_capabilities_satisfied(worker_run: &WorkerRunState) -> bool {
    let advertised = worker_capabilities();
    let missing = missing_required_capabilities(&worker_run.required_capabilities, &advertised);
    if !missing.is_empty() {
        warn!(
            worker_run_id = %worker_run.id,
            required_capabilities = %worker_run.required_capabilities,
            worker_capabilities = %advertised.join(","),
            missing_capabilities = %missing.join(","),
            "WorkerRun requires capabilities this worker does not advertise"
        );
        return false;
    }
    true
}

fn worker_capabilities() -> Vec<String> {
    env::var("PAW_CODEX_WORKER_CAPABILITIES")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            "local_codex,repo_write,review,evaluation,datadog_query,github_query,directed_evolution,variant_generation,simulated_user".to_string()
        })
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn missing_required_capabilities(required_capabilities: &str, worker_capabilities: &[String]) -> Vec<String> {
    let advertised = worker_capabilities
        .iter()
        .map(|capability| capability.trim().to_ascii_lowercase())
        .collect::<std::collections::HashSet<_>>();
    required_capabilities
        .split(',')
        .map(str::trim)
        .filter(|capability| !capability.is_empty())
        .filter(|capability| !advertised.contains(&capability.to_ascii_lowercase()))
        .map(str::to_string)
        .collect()
}
