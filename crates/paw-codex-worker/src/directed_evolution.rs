async fn handle_queued_directed_evolution_work_item(
    client: &reqwest::Client,
    config: &Config,
    work_item_id: &str,
) -> Result<()> {
    info!(work_item_id, "saw queued Directed Evolution WorkItem");
    let work_item = fetch_directed_evolution_work_item(client, config, work_item_id).await?;
    if work_item.status != "Queued" {
        debug!(
            work_item_id,
            status = %work_item.status,
            "Directed Evolution WorkItem no longer queued"
        );
        return Ok(());
    }
    if work_item.role.trim().is_empty() {
        debug!(work_item_id, "Directed Evolution WorkItem has no brain role yet");
        return Ok(());
    }
    if let Some(reason) =
        stale_directed_evolution_work_item_reason(client, config, &work_item).await?
    {
        eliminate_stale_directed_evolution_stage_result(client, config, &work_item, &reason)
            .await?;
        post_directed_evolution_action(
            client,
            config,
            "WorkItems",
            &work_item.id,
            "CancelWorkItem",
            json!({ "Reason": reason }),
        )
        .await?;
        info!(
            work_item_id = %work_item.id,
            role = %work_item.role,
            "cancelled stale Directed Evolution WorkItem"
        );
        return Ok(());
    }

    let brain_run_id = create_entity(client, config, "BrainRuns", json!({})).await?;
    post_directed_evolution_action(
        client,
        config,
        "WorkItems",
        &work_item.id,
        "ClaimWorkItem",
        json!({
            "WorkerId": config.worker_id,
            "ClaimedBy": config.worker_id,
        }),
    )
    .await?;
    post_directed_evolution_action(
        client,
        config,
        "BrainRuns",
        &brain_run_id,
        "StartBrainRun",
        json!({
            "Role": work_item.role,
            "WorkItemId": work_item.id,
            "AgentKind": directed_evolution_agent_kind_for_role(&work_item.role),
            "Model": directed_evolution_model_for_role(&work_item.role),
            "ParentSessionId": env::var("CODEX_SESSION_ID").unwrap_or_default(),
            "CorrelationJson": work_item.correlation_json,
        }),
    )
    .await?;
    post_directed_evolution_action(
        client,
        config,
        "WorkItems",
        &work_item.id,
        "StartWorkItem",
        json!({ "BrainRunId": brain_run_id }),
    )
    .await?;
    info!(
        work_item_id = %work_item.id,
        brain_run_id = %brain_run_id,
        role = %work_item.role,
        target_entity_type = %work_item.target_entity_type,
        target_entity_id = %work_item.target_entity_id,
        "started Directed Evolution worker run"
    );

    match run_directed_evolution_codex_role(client, config, &work_item).await {
        Ok(output_json) => {
            let summary = directed_evolution_summary(&work_item, &output_json);
            let evidence_artifact_id = record_directed_evolution_brain_evidence(
                client,
                config,
                &work_item,
                &brain_run_id,
                "codex_brain_run",
                &output_json,
                &summary,
            )
            .await?;
            post_directed_evolution_action(
                client,
                config,
                "BrainRuns",
                &brain_run_id,
                "SucceedBrainRun",
                json!({
                    "OutputJson": output_json,
                    "EvidenceArtifactId": evidence_artifact_id,
                    "Summary": summary,
                }),
            )
            .await?;
            post_directed_evolution_action(
                client,
                config,
                "WorkItems",
                &work_item.id,
                "SucceedWorkItem",
                json!({
                    "ResultJson": output_json,
                    "EvidenceArtifactId": evidence_artifact_id,
                    "Summary": summary,
                }),
            )
            .await?;
            info!(
                work_item_id = %work_item.id,
                brain_run_id = %brain_run_id,
                role = %work_item.role,
                evidence_artifact_id = %evidence_artifact_id,
                "completed Directed Evolution Codex brain run"
            );
            Ok(())
        }
        Err(error) => {
            let failure_reason = format!("Directed Evolution Codex role failed: {error}");
            let evidence_artifact_id = match record_directed_evolution_brain_evidence(
                client,
                config,
                &work_item,
                &brain_run_id,
                "codex_brain_run_failure",
                &serde_json::to_string(&json!({
                    "status": "failed",
                    "failure_reason": failure_reason,
                }))?,
                &failure_reason,
            )
            .await
            {
                Ok(id) => id,
                Err(report_error) => {
                    warn!(%report_error, work_item_id, brain_run_id, "failed to record Directed Evolution failure evidence");
                    String::new()
                }
            };
            if let Err(report_error) = post_directed_evolution_action(
                client,
                config,
                "BrainRuns",
                &brain_run_id,
                "FailBrainRun",
                json!({
                    "FailureReason": failure_reason,
                    "EvidenceArtifactId": evidence_artifact_id,
                }),
            )
            .await
            {
                warn!(%report_error, work_item_id, brain_run_id, "failed to report BrainRun failure");
            }
            post_directed_evolution_action(
                client,
                config,
                "WorkItems",
                &work_item.id,
                "FailWorkItem",
                json!({
                    "FailureReason": failure_reason,
                    "EvidenceArtifactId": evidence_artifact_id,
                }),
            )
            .await?;
            warn!(
                work_item_id = %work_item.id,
                brain_run_id = %brain_run_id,
                role = %work_item.role,
                evidence_artifact_id = %evidence_artifact_id,
                "failed Directed Evolution Codex brain run"
            );
            Ok(())
        }
    }
}

async fn stale_directed_evolution_work_item_reason(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
) -> Result<Option<String>> {
    if !directed_evolution_stage_evaluator_role(&work_item.role)
        || work_item.target_entity_type != "StageResult"
    {
        return Ok(None);
    }

    let stage_fields =
        fetch_directed_evolution_entity_fields(client, config, "StageResults", &work_item.target_entity_id)
            .await?;
    let variant_fields = {
        let variant_id = value_field_string(&stage_fields, &["VariantId", "variant_id"]);
        if variant_id.trim().is_empty() {
            json!({})
        } else {
            fetch_directed_evolution_entity_fields(client, config, "Variants", &variant_id).await?
        }
    };
    Ok(stale_directed_evolution_stage_work_reason(
        work_item,
        &stage_fields,
        &variant_fields,
    ))
}

async fn eliminate_stale_directed_evolution_stage_result(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    reason: &str,
) -> Result<()> {
    if !stale_stage_work_targets_stage_result(work_item) {
        return Ok(());
    }

    let stage_fields =
        fetch_directed_evolution_entity_fields(client, config, "StageResults", &work_item.target_entity_id)
            .await?;
    if !stale_stage_result_should_eliminate(&stage_fields) {
        return Ok(());
    }

    post_directed_evolution_action(
        client,
        config,
        "StageResults",
        &work_item.target_entity_id,
        "EliminateStageResult",
        json!({
            "EliminationRuleId": "stale-after-variant-terminal",
            "EvidenceArtifactId": value_field_string(&stage_fields, &["EvidenceArtifactId", "evidence_artifact_id"]),
            "Reason": reason,
        }),
    )
    .await
}

fn stale_directed_evolution_stage_work_reason(
    work_item: &DirectedEvolutionWorkItemState,
    stage_fields: &Value,
    variant_fields: &Value,
) -> Option<String> {
    if !stale_stage_work_targets_stage_result(work_item) {
        return None;
    }
    let stage_status = value_field_string(stage_fields, &["Status", "status"]);
    if !stage_status.trim().is_empty() && stage_status != "Running" {
        return Some(format!(
            "Target StageResult {} is already {}; skipping stale {} work",
            work_item.target_entity_id, stage_status, work_item.role
        ));
    }
    let variant_status = value_field_string(variant_fields, &["Status", "status"]);
    if matches!(
        variant_status.as_str(),
        "Eliminated" | "Promoted" | "Superseded" | "Failed"
    ) {
        return Some(format!(
            "Target variant is already {}; skipping stale {} work for StageResult {}",
            variant_status, work_item.role, work_item.target_entity_id
        ));
    }
    None
}

fn stale_stage_work_targets_stage_result(work_item: &DirectedEvolutionWorkItemState) -> bool {
    directed_evolution_stage_evaluator_role(&work_item.role)
        && work_item.target_entity_type == "StageResult"
}

fn directed_evolution_stage_evaluator_role(role: &str) -> bool {
    matches!(
        role,
        "reviewer"
            | "viability_evaluator"
            | "state_verifier"
            | "telemetry_evaluator"
            | "wasm_evaluator"
    )
}

fn stale_stage_result_should_eliminate(stage_fields: &Value) -> bool {
    matches!(
        value_field_string(stage_fields, &["Status", "status"]).as_str(),
        "Running" | "Failed"
    )
}


include!("directed_evolution/evidence.rs");
include!("directed_evolution/observer_sources.rs");
include!("directed_evolution/human_episode_defaults.rs");
include!("directed_evolution/human_episode_plan.rs");
include!("directed_evolution/human_episode.rs");

async fn post_directed_evolution_action(
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
        DIRECTED_EVOLUTION_NAMESPACE,
        action,
        body,
    )
    .await
}

async fn run_directed_evolution_codex_role(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
) -> Result<String> {
    let mut prompt = directed_evolution_prompt(work_item);
    info!(
        work_item_id = %work_item.id,
        role = %work_item.role,
        target_entity_type = %work_item.target_entity_type,
        target_entity_id = %work_item.target_entity_id,
        execution_enabled = config.enable_execution,
        "executing Directed Evolution Codex role"
    );
    if !config.enable_execution {
        return serde_json::to_string(&json!({
            "status": "dry_run",
            "role": work_item.role,
            "work_item_id": work_item.id,
            "target": {
                "entity_type": work_item.target_entity_type,
                "entity_id": work_item.target_entity_id,
            },
            "prompt_preview": truncate_middle(&prompt, 1200),
        }))
            .context("serialize Directed Evolution dry-run output");
    }
    if directed_evolution_mechanical_evaluator_role(&work_item.role) {
        let payload = run_directed_evolution_mechanical_evaluator(client, config, work_item).await?;
        return serde_json::to_string(&payload)
            .context("serialize Directed Evolution mechanical evaluator output");
    }
    if work_item.role == "promoter" {
        let materialization =
            materialize_directed_evolution_promotion(client, config, work_item).await?;
        return serde_json::to_string(&directed_evolution_promotion_output(&materialization))
            .context("serialize Directed Evolution promoter output");
    }

    let workdir = resolve_directed_evolution_workdir(client, config, work_item).await?;
    if work_item.role == "observer" {
        let inventory =
            directed_evolution_observer_source_inventory_prompt(client, config, work_item, &workdir)
                .await?;
        prompt.push_str(
            "\n\nObserver source inventory:\n\
The following JSON is a source map, not a script. Use it to orient yourself, then inspect any \
additional available source that could confirm, contradict, or refine the observation.\n",
        );
        prompt.push_str(&inventory);
        prompt.push_str(
            "\n\nObserver source-discovery discipline:\n\
- Do not limit yourself to the seed queries or samples in the inventory.\n\
- Include every important source you used, every important zero-result source, and every important unavailable source in evidence_scope.\n\
- Prefer conclusions supported by multiple surfaces, especially runtime state plus telemetry or stored trajectory evidence.\n",
        );
    }
    let readonly_status_before = if directed_evolution_role_may_write_repo(&work_item.role) {
        None
    } else {
        directed_evolution_git_status_snapshot(&workdir.path).await?
    };
    let output = match run_codex_exec_command(
        config,
        &workdir.path,
        prompt,
        "run Directed Evolution Codex role",
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            return recover_directed_evolution_variant_output(
                client, config, work_item, &workdir, error,
            )
            .await;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return recover_directed_evolution_variant_output(
            client,
            config,
            work_item,
            &workdir,
            anyhow!(
                "codex role {} failed with status {:?}: {}",
                work_item.role,
                output.status.code(),
                truncate_middle(&format!("{stdout}\n{stderr}"), 4_000)
            ),
        )
        .await;
    }
    let mut payload = parse_codex_jsonish(&stdout).unwrap_or_else(|| {
        json!({
            "status": "succeeded",
            "summary": truncate_middle(&stdout, 4_000),
        })
    });
    if let Some(status_before) = readonly_status_before {
        ensure_directed_evolution_readonly_workdir_unchanged(&workdir.path, &status_before).await?;
    }
    payload = finalize_directed_evolution_output(client, config, work_item, &workdir, payload).await?;
    if let Some(object) = payload.as_object_mut() {
        object
            .entry("role".to_string())
            .or_insert_with(|| json!(work_item.role));
        object
            .entry("work_item_id".to_string())
            .or_insert_with(|| json!(work_item.id));
        object.entry("target".to_string()).or_insert_with(|| {
            json!({
                "entity_type": work_item.target_entity_type,
                "entity_id": work_item.target_entity_id,
            })
        });
        object.entry("execution".to_string()).or_insert_with(|| {
            json!({
                "workdir": workdir.path.display().to_string(),
                "stdout_bytes": stdout.len(),
                "stderr_bytes": stderr.len(),
            })
        });
    }
    serde_json::to_string(&payload)
    .context("serialize Directed Evolution Codex output")
}

fn directed_evolution_agent_kind_for_role(role: &str) -> &'static str {
    if matches!(role, "promoter" | "state_verifier" | "wasm_evaluator") {
        "temperpaw-worker"
    } else {
        "codex"
    }
}

fn directed_evolution_model_for_role(role: &str) -> &'static str {
    if matches!(role, "promoter" | "state_verifier" | "wasm_evaluator") {
        "deterministic-worker"
    } else {
        "codex-cli"
    }
}

async fn recover_directed_evolution_variant_output(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    workdir: &DirectedEvolutionWorkdir,
    error: anyhow::Error,
) -> Result<String> {
    if work_item.role != "variant_generator"
        || env_flag("PAW_DE_RECOVER_VARIANT_CHANGES_ON_CODEX_ERROR") == Some(false)
    {
        return Err(error);
    }
    let changed_files = match git_changed_files(&workdir.path).await {
        Ok(files) if !files.is_empty() => files,
        _ => return Err(error),
    };
    let error_summary = truncate_middle(&error.to_string(), 1_200);
    warn!(
        work_item_id = %work_item.id,
        changed_file_count = changed_files.len(),
        error = %error_summary,
        "recovering Directed Evolution variant from git changes after Codex execution error"
    );
    let payload = json!({
        "status": "recovered_after_codex_exec_error",
        "summary": format!(
            "Recovered a Directed Evolution variant from {} changed file(s) after Codex exited before returning JSON.",
            changed_files.len()
        ),
        "changed_files": changed_files,
        "verification_notes": format!(
            "Codex produced repository changes but did not finish the structured response: {error_summary}. Later evaluation stages must validate the hot-loaded runtime before selection."
        ),
        "reasoning_summary": "The worker recovered concrete file mutations left by the variant-generator brain and will publish them as a candidate for downstream evaluation.",
        "recovery": {
            "reason": "codex_exec_error_after_file_changes",
            "error": error_summary,
            "worker_id": config.worker_id,
        }
    });
    let payload = finalize_directed_evolution_output(client, config, work_item, workdir, payload)
        .await?;
    serde_json::to_string(&payload).context("serialize recovered Directed Evolution variant output")
}


include!("directed_evolution/workdir.rs");
include!("directed_evolution/mechanical_evaluator.rs");
include!("directed_evolution/prompt.rs");
include!("directed_evolution/tests.rs");
include!("directed_evolution/prompt_tests.rs");
