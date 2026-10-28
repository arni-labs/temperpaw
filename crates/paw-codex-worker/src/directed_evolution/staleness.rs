// Staleness and role-guard checks for Directed Evolution work items:
// stage-evaluator targeting, stale stage-result elimination, and runtime
// credential preflight guards. Included into main.rs's flat namespace via
// directed_evolution.rs (see the include! block there).

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
    observe_metadata: Option<&str>,
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
        observe_metadata,
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

/// Cheap dispatch assertion: simulated users exercise Trials, evaluator
/// roles judge StageResults. A mismatch is a control-plane routing bug and
/// must fail the work item with a clear reason instead of confusing the
/// downstream brain.
fn directed_evolution_role_target_mismatch(
    role: &str,
    target_entity_type: &str,
) -> Option<String> {
    let expected = if role == "simulated_user" {
        "Trial"
    } else if directed_evolution_stage_evaluator_role(role) {
        "StageResult"
    } else {
        return None;
    };
    if target_entity_type == expected {
        return None;
    }
    Some(format!(
        "Directed Evolution role {role} requires a {expected} target, got {target_entity_type}"
    ))
}

/// Codex roles that exercise a variant runtime with bearer credentials.
/// The observer is excluded: it resolves runtime auth best-effort inside
/// its source inventory and degrades to other sources. Mechanical
/// evaluator roles never launch Codex and never authenticate.
fn directed_evolution_runtime_credential_role(role: &str) -> bool {
    matches!(
        role,
        "simulated_user" | "reviewer" | "viability_evaluator" | "telemetry_evaluator"
    )
}

/// B9 runtime auth preflight: when a work item's correlation names a
/// runtime to exercise, verify at least one of the runtime auth env vars
/// is set before launching Codex. Only env var NAMES are resolved and
/// reported — never values. A missing credential otherwise surfaces as a
/// confusing 401 inside the Codex child.
fn directed_evolution_runtime_credential_failure(
    role: &str,
    join: &DirectedEvolutionJoinFields,
    env_value: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if !directed_evolution_runtime_credential_role(role) {
        return None;
    }
    if join.runtime_base_url.trim().is_empty() {
        return None;
    }
    let names = directed_evolution_runtime_auth_env_var_names(&join.runtime_auth_env_vars);
    let any_set = names.iter().any(|name| {
        env_value(name)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
    });
    if any_set {
        return None;
    }
    Some(format!(
        "runtime credential missing: none of [{}] is set",
        names.join(", ")
    ))
}

fn directed_evolution_runtime_auth_env_var_names(configured: &[String]) -> Vec<String> {
    let mut names: Vec<String> = configured
        .iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    for fallback in ["TEMPERPAW_RUNTIME_API_KEY", "TEMPER_API_KEY"] {
        if !names.iter().any(|name| name == fallback) {
            names.push(fallback.to_string());
        }
    }
    names
}

fn stale_stage_result_should_eliminate(stage_fields: &Value) -> bool {
    matches!(
        value_field_string(stage_fields, &["Status", "status"]).as_str(),
        "Running" | "Failed"
    )
}

