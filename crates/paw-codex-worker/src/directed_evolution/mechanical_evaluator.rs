fn directed_evolution_mechanical_evaluator_role(role: &str) -> bool {
    matches!(role, "state_verifier" | "wasm_evaluator")
}

async fn run_directed_evolution_mechanical_evaluator(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
) -> Result<Value> {
    // Target-type integrity is asserted once at role dispatch
    // (directed_evolution_role_target_mismatch) before this runs.
    let stage_result =
        fetch_directed_evolution_entity_fields(client, config, "StageResults", &work_item.target_entity_id)
            .await?;
    let variant_id = value_field_string(&stage_result, &["VariantId", "variant_id"]);
    let evaluation_stage_id =
        value_field_string(&stage_result, &["EvaluationStageId", "evaluation_stage_id"]);
    let variant = if variant_id.trim().is_empty() {
        json!({})
    } else {
        fetch_directed_evolution_entity_fields(client, config, "Variants", &variant_id)
            .await
            .unwrap_or_else(|error| json!({ "FetchError": error.to_string() }))
    };
    let mutation_id = value_field_string(&variant, &["MutationId", "mutation_id"]);
    let mutation = if mutation_id.trim().is_empty() {
        json!({})
    } else {
        fetch_directed_evolution_entity_fields(client, config, "Mutations", &mutation_id)
            .await
            .unwrap_or_else(|error| json!({ "FetchError": error.to_string() }))
    };
    let evaluation_stage = if evaluation_stage_id.trim().is_empty() {
        json!({})
    } else {
        fetch_directed_evolution_entity_fields(
            client,
            config,
            "EvaluationStages",
            &evaluation_stage_id,
        )
        .await
        .unwrap_or_else(|error| json!({ "FetchError": error.to_string() }))
    };

    Ok(directed_evolution_state_verifier_output(
        work_item,
        &stage_result,
        &variant,
        &mutation,
        &evaluation_stage,
    ))
}

fn directed_evolution_state_verifier_output(
    work_item: &DirectedEvolutionWorkItemState,
    stage_result: &Value,
    variant: &Value,
    mutation: &Value,
    evaluation_stage: &Value,
) -> Value {
    let variant_id = value_field_string(stage_result, &["VariantId", "variant_id"]);
    let mutation_id = value_field_string(variant, &["MutationId", "mutation_id"]);
    let changed_files = directed_evolution_changed_files(mutation)
        .or_else(|| directed_evolution_changed_files(variant))
        .unwrap_or_default();
    let evaluator_module = nonempty(
        value_field_string(evaluation_stage, &["EvaluatorModule", "evaluator_module"]),
        if work_item.role == "wasm_evaluator" {
            "agent_answers_state_verifier".to_string()
        } else {
            "state-verifier".to_string()
        },
    );
    let evaluator_ref = nonempty(
        value_field_string(evaluation_stage, &["EvaluatorRef", "evaluator_ref"]),
        "genesis://nerdsane/agent-answers-evaluation@frozen".to_string(),
    );
    let role = if work_item.role == "wasm_evaluator" {
        "wasm_evaluator"
    } else {
        "state_verifier"
    };
    let provenance = if work_item.role == "wasm_evaluator" {
        "wasm-computed"
    } else {
        "state-verified"
    };

    let mut findings = Vec::new();
    if variant_id.trim().is_empty() {
        findings.push("StageResult has no VariantId.".to_string());
    }
    if mutation_id.trim().is_empty() {
        findings.push("Variant has no MutationId; no concrete diff is available.".to_string());
    }
    if changed_files.is_empty() {
        findings.push("Mutation has no changed files.".to_string());
    }
    if let Some(error) = variant.get("FetchError").and_then(Value::as_str) {
        findings.push(format!("Variant fetch failed: {error}"));
    }
    if let Some(error) = mutation.get("FetchError").and_then(Value::as_str) {
        findings.push(format!("Mutation fetch failed: {error}"));
    }
    if let Some(error) = evaluation_stage.get("FetchError").and_then(Value::as_str) {
        findings.push(format!("EvaluationStage fetch failed: {error}"));
    }

    let evaluator_boundary_violations: Vec<String> = changed_files
        .iter()
        .filter(|path| directed_evolution_evaluator_path(path))
        .cloned()
        .collect();
    if !evaluator_boundary_violations.is_empty() {
        findings.push(format!(
            "Variant modified pinned evaluator files: {}",
            evaluator_boundary_violations.join(", ")
        ));
    }

    let out_of_bundle_changes: Vec<String> = changed_files
        .iter()
        .filter(|path| !directed_evolution_agent_answers_app_path(path))
        .cloned()
        .collect();
    if !out_of_bundle_changes.is_empty() {
        findings.push(format!(
            "Variant changed files outside the Agent Answers app bundle: {}",
            out_of_bundle_changes.join(", ")
        ));
    }

    let passed = findings.is_empty();
    let viability_regression_count = if passed { 0 } else { findings.len() };
    let summary = if passed {
        format!(
            "State verifier accepted variant {variant_id}: {} changed file(s) stayed inside the Agent Answers organism and did not modify the pinned evaluator.",
            changed_files.len()
        )
    } else {
        format!(
            "State verifier rejected variant {variant_id}: {}.",
            findings.join(" ")
        )
    };
    let failure_reason = if passed {
        String::new()
    } else {
        findings.join(" ")
    };

    json!({
        "passed": passed,
        "status": if passed { "passed" } else { "failed" },
        "summary": summary,
        "failure_reason": failure_reason,
        "evaluator_role": role,
        "provenance_kind": provenance,
        "metrics": {
            "viability_regression_count": {
                "value": viability_regression_count,
                "unit": "count",
                "provenance_kind": provenance,
                "interpretation": "Deterministic state verifier count of missing mutation/diff data, evaluator-boundary changes, and out-of-bundle changes."
            },
            "mutation_file_count": {
                "value": changed_files.len(),
                "unit": "files",
                "provenance_kind": provenance,
                "interpretation": "Number of concrete files recorded on the Mutation entity."
            },
            "evaluator_boundary_violation_count": {
                "value": evaluator_boundary_violations.len(),
                "unit": "files",
                "provenance_kind": provenance,
                "interpretation": "Number of changed files under the pinned evaluator bundle."
            }
        },
        "decision_basis": {
            "why": summary,
            "findings": findings,
            "allowed_bundle": "apps/agent-answers/ or canonical app-root bundle paths",
            "forbidden_evaluator_bundle": "apps/agent-answers-evaluation/ or canonical evaluator-root paths"
        },
        "inputs": {
            "stage_result": stage_result,
            "variant": variant,
            "mutation": mutation,
            "evaluation_stage": evaluation_stage,
            "evaluator_ref": evaluator_ref,
            "evaluator_module": evaluator_module
        },
        "evidence_scope": [
            {
                "surface": "state",
                "query": format!(
                    "StageResult({}) -> Variant({}) -> Mutation({})",
                    work_item.target_entity_id,
                    variant_id,
                    mutation_id
                ),
                "time_window": "entity-state-at-evaluation",
                "result_count": changed_files.len(),
                "interpretation": summary,
                "zero_result_meaning": if changed_files.is_empty() { "failure" } else { "neutral" },
                "evidence_provenance": provenance,
                "artifact_kind": "mechanical-state-verification"
            }
        ],
        "evidence_refs": [
            format!("temper://directed-evolution/stage-result/{}", work_item.target_entity_id)
        ],
        "reasoning_summary": summary
    })
}

fn directed_evolution_changed_files(fields: &Value) -> Option<Vec<String>> {
    let raw = value_field_string(fields, &["ChangedFilesJson", "changed_files_json", "changed_files"]);
    if raw.trim().is_empty() {
        return None;
    }
    if let Ok(values) = serde_json::from_str::<Vec<String>>(&raw) {
        return Some(values.into_iter().filter(|value| !value.trim().is_empty()).collect());
    }
    if let Ok(values) = serde_json::from_str::<Vec<Value>>(&raw) {
        return Some(
            values
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .filter(|value| !value.trim().is_empty())
                .collect(),
        );
    }
    Some(
        raw.lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn directed_evolution_evaluator_path(path: &str) -> bool {
    let normalized = path.trim_start_matches("./");
    normalized.starts_with("apps/agent-answers-evaluation/")
        || normalized.contains("/apps/agent-answers-evaluation/")
        || normalized.starts_with("agent-answers-evaluation/")
        || normalized.contains("/agent-answers-evaluation/")
}

fn directed_evolution_agent_answers_app_path(path: &str) -> bool {
    let normalized = path.trim_start_matches("./");
    normalized.starts_with("apps/agent-answers/")
        || normalized.contains("/apps/agent-answers/")
        || matches!(
            normalized,
            "APP.md" | "app.toml" | "README.md" | ".gitignore"
        )
        || normalized.starts_with("adrs/")
        || normalized.starts_with("fixtures/")
        || normalized.starts_with("policies/")
        || normalized.starts_with("specs/")
        || normalized.starts_with("wasm/")
}
