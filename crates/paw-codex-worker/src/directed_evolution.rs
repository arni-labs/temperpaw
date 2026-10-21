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
    let join_fields = directed_evolution_join_fields(&work_item.correlation_json);
    let observe_metadata_pre_run =
        directed_evolution_work_item_observe_metadata(&work_item, "", &join_fields);
    if let Some(reason) =
        stale_directed_evolution_work_item_reason(client, config, &work_item).await?
    {
        eliminate_stale_directed_evolution_stage_result(
            client,
            config,
            &work_item,
            &reason,
            Some(&observe_metadata_pre_run),
        )
        .await?;
        post_paw_orchestration_action(
            client,
            config,
            "WorkItems",
            &work_item.id,
            "CancelWorkItem",
            json!({ "Reason": reason }),
            Some(&observe_metadata_pre_run),
        )
        .await?;
        info!(
            work_item_id = %work_item.id,
            role = %work_item.role,
            "cancelled stale Directed Evolution WorkItem"
        );
        return Ok(());
    }

    let worker_run_id = create_entity_with_observe_metadata(
        client,
        config,
        "WorkerRuns",
        json!({}),
        Some(&observe_metadata_pre_run),
    )
    .await?;
    let observe_metadata =
        directed_evolution_work_item_observe_metadata(&work_item, &worker_run_id, &join_fields);
    post_paw_orchestration_action(
        client,
        config,
        "WorkItems",
        &work_item.id,
        "ClaimWorkItem",
        json!({
            "WorkerId": config.worker_id,
            "ClaimedBy": config.worker_id,
        }),
        Some(&observe_metadata),
    )
    .await?;
    post_paw_orchestration_action(
        client,
        config,
        "WorkerRuns",
        &worker_run_id,
        "StartWorkerRun",
        directed_evolution_start_worker_run_body(
            &work_item,
            &config.worker_id,
            &worker_run_id,
            &env::var("CODEX_SESSION_ID").unwrap_or_default(),
        ),
        Some(&observe_metadata),
    )
    .await?;
    post_paw_orchestration_action(
        client,
        config,
        "WorkItems",
        &work_item.id,
        "StartWorkItem",
        directed_evolution_start_work_item_body(&worker_run_id),
        Some(&observe_metadata),
    )
    .await?;
    info!(
        work_item_id = %work_item.id,
        worker_run_id = %worker_run_id,
        role = %work_item.role,
        target_entity_type = %work_item.target_entity_type,
        target_entity_id = %work_item.target_entity_id,
        "started Directed Evolution worker run"
    );

    match run_directed_evolution_codex_role(client, config, &work_item, &worker_run_id).await {
        Ok(output_json) => {
            let summary = directed_evolution_summary(&work_item, &output_json);
            let evidence_artifact_id = record_directed_evolution_worker_evidence(
                client,
                config,
                &work_item,
                &worker_run_id,
                "codex_worker_run",
                &output_json,
                &summary,
                Some(&observe_metadata),
            )
            .await?;
            post_paw_orchestration_action(
                client,
                config,
                "WorkerRuns",
                &worker_run_id,
                "SucceedWorkerRun",
                json!({
                    "OutputJson": output_json,
                    "EvidenceArtifactId": evidence_artifact_id,
                    "Summary": summary,
                }),
                Some(&observe_metadata),
            )
            .await?;
            // Receipt routing is best-effort, mirroring the failure path: a
            // routing error must not leave the WorkItem stranded in Running
            // with a succeeded WorkerRun. ResultJson still lands on the
            // WorkItem via SucceedWorkItem below.
            let receipt_id = match route_directed_evolution_success_receipt(
                client,
                config,
                &work_item,
                &worker_run_id,
                &output_json,
                &evidence_artifact_id,
                &summary,
                Some(&observe_metadata),
            )
            .await
            {
                Ok(receipt_id) => receipt_id,
                Err(report_error) => {
                    warn!(%report_error, work_item_id, worker_run_id, "failed to route Directed Evolution success receipt");
                    String::new()
                }
            };
            post_paw_orchestration_action(
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
                Some(&observe_metadata),
            )
            .await?;
            info!(
                work_item_id = %work_item.id,
                worker_run_id = %worker_run_id,
                role = %work_item.role,
                evidence_artifact_id = %evidence_artifact_id,
                receipt_id = %receipt_id,
                "completed Directed Evolution Codex worker run"
            );
            Ok(())
        }
        Err(error) => {
            let failure_reason = format!("Directed Evolution Codex role failed: {error}");
            let evidence_artifact_id = match record_directed_evolution_worker_evidence(
                client,
                config,
                &work_item,
                &worker_run_id,
                "codex_worker_run_failure",
                &serde_json::to_string(&json!({
                    "status": "failed",
                    "failure_reason": failure_reason,
                }))?,
                &failure_reason,
                Some(&observe_metadata),
            )
            .await
            {
                Ok(id) => id,
                Err(report_error) => {
                    warn!(%report_error, work_item_id, worker_run_id, "failed to record Directed Evolution failure evidence");
                    String::new()
                }
            };
            if let Err(report_error) = post_paw_orchestration_action(
                client,
                config,
                "WorkerRuns",
                &worker_run_id,
                "FailWorkerRun",
                json!({
                    "FailureReason": failure_reason,
                    "EvidenceArtifactId": evidence_artifact_id,
                }),
                Some(&observe_metadata),
            )
            .await
            {
                warn!(%report_error, work_item_id, worker_run_id, "failed to report WorkerRun failure");
            }
            if let Err(report_error) = route_directed_evolution_failure_receipt(
                client,
                config,
                &work_item,
                &worker_run_id,
                &failure_reason,
                &evidence_artifact_id,
                Some(&observe_metadata),
            )
            .await
            {
                warn!(%report_error, work_item_id, worker_run_id, "failed to route Directed Evolution failure receipt");
            }
            post_paw_orchestration_action(
                client,
                config,
                "WorkItems",
                &work_item.id,
                "FailWorkItem",
                json!({
                    "FailureReason": failure_reason,
                    "EvidenceArtifactId": evidence_artifact_id,
                }),
                Some(&observe_metadata),
            )
            .await?;
            warn!(
                work_item_id = %work_item.id,
                worker_run_id = %worker_run_id,
                role = %work_item.role,
                evidence_artifact_id = %evidence_artifact_id,
                "failed Directed Evolution Codex worker run"
            );
            Ok(())
        }
    }
}


include!("directed_evolution/observe_metadata.rs");
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
    observe_metadata: Option<&str>,
) -> Result<()> {
    post_entity_action_with_namespace_observed(
        client,
        config,
        entity_set,
        entity_id,
        DIRECTED_EVOLUTION_NAMESPACE,
        action,
        body,
        observe_metadata,
    )
    .await
}

async fn post_paw_orchestration_action(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    entity_id: &str,
    action: &str,
    body: Value,
    observe_metadata: Option<&str>,
) -> Result<()> {
    post_entity_action_with_namespace_observed(
        client,
        config,
        entity_set,
        entity_id,
        PAW_ORCHESTRATION_NAMESPACE,
        action,
        body,
        observe_metadata,
    )
    .await
}

async fn run_directed_evolution_codex_role(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
) -> Result<String> {
    if let Some(reason) =
        directed_evolution_role_target_mismatch(&work_item.role, &work_item.target_entity_type)
    {
        bail!("{reason}");
    }
    let join_fields = directed_evolution_join_fields(&work_item.correlation_json);
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
    if let Some(reason) = directed_evolution_runtime_credential_failure(
        &work_item.role,
        &join_fields,
        |name| env::var(name).ok(),
    ) {
        bail!("{reason}");
    }

    let workdir = resolve_directed_evolution_workdir(client, config, work_item).await?;
    if work_item.role == "observer" {
        let observe_metadata =
            directed_evolution_work_item_observe_metadata(work_item, worker_run_id, &join_fields);
        let inventory = directed_evolution_observer_source_inventory_prompt(
            client,
            config,
            work_item,
            &workdir,
            &observe_metadata,
        )
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
    // ADR-0041: the Codex child inherits the correlation context mechanically
    // (TEMPER_OBSERVE_METADATA + DD_TAGS), not just via prompt text.
    let child_env = directed_evolution_codex_child_env(
        work_item,
        worker_run_id,
        &join_fields,
        env::var("DD_TAGS").ok().as_deref(),
    );
    let output = match run_codex_exec_command_with_env(
        config,
        &workdir.path,
        prompt,
        "run Directed Evolution Codex role",
        &child_env,
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


include!("directed_evolution/staleness.rs");
include!("directed_evolution/receipts.rs");
include!("directed_evolution/workdir.rs");
include!("directed_evolution/mechanical_evaluator.rs");
include!("directed_evolution/prompt.rs");
include!("directed_evolution/tests.rs");
include!("directed_evolution/prompt_tests.rs");
include!("directed_evolution/observe_metadata_tests.rs");
