// WorkerRun/WorkItem receipt bodies, receipt routing, and boot-time
// recovery of Running Directed Evolution work items after a worker
// restart. Included into main.rs's flat namespace via directed_evolution.rs.

fn directed_evolution_start_worker_run_body(
    work_item: &DirectedEvolutionWorkItemState,
    worker_id: &str,
    worker_run_id: &str,
    parent_session_id: &str,
) -> Value {
    json!({
        "Role": work_item.role,
        "WorkItemId": work_item.id,
        "WorkerId": worker_id,
        "ProviderId": DIRECTED_EVOLUTION_WORKER_PROVIDER_ID,
        "AgentKind": directed_evolution_agent_kind_for_role(&work_item.role),
        "Model": directed_evolution_model_for_role(&work_item.role),
        "SessionId": worker_run_id,
        "ParentSessionId": parent_session_id,
        "CorrelationJson": work_item.correlation_json,
    })
}

fn directed_evolution_start_work_item_body(worker_run_id: &str) -> Value {
    json!({ "WorkerRunId": worker_run_id })
}

fn directed_evolution_success_receipt_body(
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    result_json: &str,
    evidence_artifact_id: &str,
    summary: &str,
) -> Value {
    json!({
        "WorkItemId": work_item.id,
        "Role": work_item.role,
        "TargetEntityType": work_item.target_entity_type,
        "TargetEntityId": work_item.target_entity_id,
        "WorkerRunId": worker_run_id,
        "ResultJson": result_json,
        "EvidenceArtifactId": evidence_artifact_id,
        "Summary": summary,
        "CorrelationJson": work_item.correlation_json,
    })
}

fn directed_evolution_failure_receipt_body(
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    failure_reason: &str,
    evidence_artifact_id: &str,
) -> Value {
    json!({
        "WorkItemId": work_item.id,
        "Role": work_item.role,
        "TargetEntityType": work_item.target_entity_type,
        "TargetEntityId": work_item.target_entity_id,
        "WorkerRunId": worker_run_id,
        "FailureReason": failure_reason,
        "EvidenceArtifactId": evidence_artifact_id,
        "CorrelationJson": work_item.correlation_json,
    })
}

#[allow(clippy::too_many_arguments)]
async fn route_directed_evolution_success_receipt(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    result_json: &str,
    evidence_artifact_id: &str,
    summary: &str,
    observe_metadata: Option<&str>,
) -> Result<String> {
    let receipt_id = create_entity_with_observe_metadata(
        client,
        config,
        "WorkItemReceipts",
        json!({}),
        observe_metadata,
    )
    .await?;
    post_directed_evolution_action(
        client,
        config,
        "WorkItemReceipts",
        &receipt_id,
        "RouteSucceededWorkItem",
        directed_evolution_success_receipt_body(
            work_item,
            worker_run_id,
            result_json,
            evidence_artifact_id,
            summary,
        ),
        observe_metadata,
    )
    .await?;
    Ok(receipt_id)
}

async fn route_directed_evolution_failure_receipt(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    failure_reason: &str,
    evidence_artifact_id: &str,
    observe_metadata: Option<&str>,
) -> Result<String> {
    let receipt_id = create_entity_with_observe_metadata(
        client,
        config,
        "WorkItemReceipts",
        json!({}),
        observe_metadata,
    )
    .await?;
    post_directed_evolution_action(
        client,
        config,
        "WorkItemReceipts",
        &receipt_id,
        "RouteFailedWorkItem",
        directed_evolution_failure_receipt_body(
            work_item,
            worker_run_id,
            failure_reason,
            evidence_artifact_id,
        ),
        observe_metadata,
    )
    .await?;
    Ok(receipt_id)
}

fn directed_evolution_running_recovery_filter(worker_id: &str) -> String {
    // OData escapes a single quote inside a string literal by doubling it.
    let escaped = worker_id.replace('\'', "''");
    format!("Status eq 'Running' and ClaimedBy eq '{escaped}'")
}

fn directed_evolution_restart_failure_reason(worker_id: &str) -> String {
    format!(
        "worker {worker_id} restarted while this work item was running; failed for control-plane re-dispatch"
    )
}

async fn recover_boot_running_directed_evolution_work_items(
    client: &reqwest::Client,
    config: &Config,
) -> Result<()> {
    let filter = directed_evolution_running_recovery_filter(&config.worker_id);
    let ids = query_boot_entity_ids_filtered(client, config, "WorkItems", &filter).await?;
    for work_item_id in ids {
        if let Err(error) =
            fail_recovered_directed_evolution_work_item(client, config, &work_item_id).await
        {
            warn!(%error, work_item_id, "failed to recover Running Directed Evolution WorkItem");
        }
    }
    Ok(())
}

async fn fail_recovered_directed_evolution_work_item(
    client: &reqwest::Client,
    config: &Config,
    work_item_id: &str,
) -> Result<()> {
    let work_item = fetch_directed_evolution_work_item(client, config, work_item_id).await?;
    if work_item.status != "Running" {
        return Ok(());
    }
    let join_fields = directed_evolution_join_fields(&work_item.correlation_json);
    let observe_metadata = directed_evolution_work_item_observe_metadata(
        &work_item,
        &work_item.worker_run_id,
        &join_fields,
    );
    let failure_reason = directed_evolution_restart_failure_reason(&config.worker_id);
    if !work_item.worker_run_id.trim().is_empty()
        && let Err(report_error) = post_paw_orchestration_action(
            client,
            config,
            "WorkerRuns",
            &work_item.worker_run_id,
            "FailWorkerRun",
            json!({
                "FailureReason": failure_reason,
                "EvidenceArtifactId": "",
            }),
            Some(&observe_metadata),
        )
        .await
    {
        warn!(%report_error, work_item_id, worker_run_id = %work_item.worker_run_id, "failed to fail recovered Directed Evolution WorkerRun");
    }
    if let Err(report_error) = route_directed_evolution_failure_receipt(
        client,
        config,
        &work_item,
        &work_item.worker_run_id,
        &failure_reason,
        "",
        Some(&observe_metadata),
    )
    .await
    {
        warn!(%report_error, work_item_id, "failed to route recovered Directed Evolution failure receipt");
    }
    post_paw_orchestration_action(
        client,
        config,
        "WorkItems",
        &work_item.id,
        "FailWorkItem",
        json!({
            "FailureReason": failure_reason,
            "EvidenceArtifactId": "",
        }),
        Some(&observe_metadata),
    )
    .await?;
    warn!(
        work_item_id,
        "failed Running Directed Evolution WorkItem after worker restart"
    );
    Ok(())
}
