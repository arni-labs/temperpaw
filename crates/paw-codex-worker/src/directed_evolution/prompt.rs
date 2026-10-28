fn directed_evolution_prompt(work_item: &DirectedEvolutionWorkItemState) -> String {
    let role_contract = match work_item.role.as_str() {
        "observer" => {
            "Observe real signals and infer pressures. Start from the observer source inventory, then inspect any additional accessible source that can clarify the app's current behavior. Available surfaces may include Genesis state, WorkItems, WorkerRuns, EvidenceArtifacts, Signals, Directions, runtime OData state, app source/description files, Datadog logs/traces/metrics/monitors, trajectory records, and unmet-intent/friction outputs. Datadog inspection is mandatory when Datadog credentials are available, but it is one source among the full observed system. Do not treat a signal summary or a single scripted query as proof. Return multiple candidate pressures or directions when evidence supports them, rejected interpretations, structured evidence_scope entries with query, time_window, result_count, interpretation, zero_result_meaning, datadog_url when applicable, and confidence. If important sources are unavailable, keep them in evidence_scope with an explicit zero_result_meaning or unavailable interpretation."
        }
        "direction_framer" => {
            "Frame human-legible directions from pressures. Return direction title, rationale, source signals, required human gate, and likely adaptation goals."
        }
        "variant_generator" => {
            "Generate one bounded candidate variant for the target organism. Make the concrete app-bundle changes, avoid long exploratory verification, and return the concise JSON object immediately after the mutation is complete."
        }
        "simulated_user" => {
            "Act as an AI simulated user against the target organism. Exercise the live runtime when a RuntimeRef is provided. Do not judge viability, do not pass/fail the variant, and do not select a winner. Return journeys, observations, friction, unmet intents, user-visible measurements, and evidence_scope entries from the app interaction only."
        }
        "reviewer" | "viability_evaluator" => {
            "Evaluate a variant against the Adaptation Goal, Viability Constraints, Selection Protocol, and recorded evidence. Use authenticated Datadog MCP tools only when the stage requires telemetry evidence. Return pass/fail reasoning, explicit provenance_kind, metrics, structured evidence_scope entries, and risk notes."
        }
        "state_verifier" => {
            "Run deterministic state/spec verification against the variant. Use specs, CSDL, Cedar, runtime state, and transition checks rather than subjective judgment. Return pass/fail, provenance_kind=state-verified, metrics, decision_basis, and concrete evidence."
        }
        "telemetry_evaluator" => {
            "Evaluate Datadog or runtime telemetry evidence for the variant. Query logs, traces, metrics, or monitors when available. Return pass/fail, provenance_kind=datadog-measured or runtime-measured, metrics, query summaries, result counts, zero-result meaning, and Datadog URLs when available."
        }
        "wasm_evaluator" => {
            "Run or inspect the pinned deterministic evaluator bundle for this stage. Do not let the variant alter the evaluator judging it. Return pass/fail, provenance_kind=wasm-computed, metrics, evaluator inputs, and evidence."
        }
        "selector" => {
            "Select a winner from supplied evaluated-variant evidence without changing files, evaluators, or moving goalposts. Return winner, losers, scores, and selection rationale."
        }
        "promoter" => {
            "Materialize an already-selected promotion into the canonical Genesis runtime. Do not choose a winner; publish and hot-load the selected app ref."
        }
        "narrator" => {
            "Explain the episode outcome for Mission Control. Return concise human-facing narrative, lineage impact, and evidence links."
        }
        _ => {
            "Execute the Directed Evolution worker role described by this WorkItem. Return structured evidence and next-state recommendations."
        }
    };
    let output_contract = directed_evolution_output_contract(&work_item.role);
    let prompt_body =
        directed_evolution_worker_prompt_body(&work_item.role, &literal_prompt_ref(&work_item.prompt_ref));
    let join_block = directed_evolution_prompt_join_block(&directed_evolution_join_fields(
        &work_item.correlation_json,
    ));
    format!(
        r#"You are a Codex WorkerRun executing a Directed Evolution WorkItem.

Role: {role}
WorkItemId: {work_item_id}
TargetEntityType: {target_entity_type}
TargetEntityId: {target_entity_id}
ContextRef: {context_ref}
OutputSchemaRef: {output_schema_ref}
CorrelationJson: {correlation_json}
ResolvedCorrelation:
{join_block}

Role contract:
{role_contract}

Prompt:
{prompt_body}

Return exactly one concise JSON object. Do not wrap it in Markdown.
Required output shape for this role:
{output_contract}
"#,
        role = work_item.role,
        work_item_id = work_item.id,
        target_entity_type = work_item.target_entity_type,
        target_entity_id = work_item.target_entity_id,
        context_ref = work_item.context_ref,
        output_schema_ref = work_item.output_schema_ref,
        correlation_json = work_item.correlation_json,
        join_block = join_block,
        role_contract = role_contract,
        prompt_body = prompt_body,
        output_contract = output_contract,
    )
}

fn directed_evolution_prompt_join_block(join: &DirectedEvolutionJoinFields) -> String {
    let entries = join.entries();
    if entries.is_empty() {
        return "(none)".to_string();
    }
    entries
        .iter()
        .map(|(key, value)| format!("de.{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn directed_evolution_output_contract(role: &str) -> &'static str {
    match role {
        "observer" => {
            r#"{
  "actionable": true,
  "directions": [
    {
      "pressure_class": "growth|repair|performance|policy|ux",
      "pressure_summary": "...",
      "title": "...",
      "direction_summary": "...",
      "autonomy_lane": "human-approval|repair-auto",
      "proposed_adaptation_goal": "...",
      "proposed_viability_constraints": ["..."]
    }
  ],
  "rejected_interpretations": [{"interpretation":"...","why_rejected":"..."}],
  "evidence_scope": [{"surface":"logs|traces|metrics|monitors|runtime|genesis|source","query":"...","time_window":"...","result_count":0,"interpretation":"...","zero_result_meaning":"success|failure|neutral|unavailable","datadog_url":"https://app.datadoghq.com/..."}],
  "evidence_refs": ["..."],
  "reasoning_summary": "..."
}"#
        }
        "variant_generator" => {
            r#"{
  "summary": "...",
  "app_ref": "repository-or-runtime-ref",
  "branch_ref": "branch-or-worktree-ref",
  "runtime_ref": "runnable variant URL or local ref",
  "changed_files": ["..."],
  "diff_ref": "...",
  "verification_notes": "...",
  "reasoning_summary": "..."
}"#
        }
        "simulated_user" => {
            r#"{
  "status": "observed|blocked",
  "summary": "...",
  "journey": [{"step":"...","result":"..."}],
  "observations": {"what_happened":"...","unmet_intents":["..."]},
  "intent_satisfied": "yes|partial|no",
  "friction": ["..."],
  "metrics": {"observed_latency_ms":{"value":0,"unit":"ms","provenance_kind":"agent-observed"}},
  "evidence_scope": [{"surface":"runtime|app","query":"...","result_summary":"..."}],
  "evidence_refs": ["..."],
  "blocker": "",
  "reasoning_summary": "..."
}"#
        }
        "reviewer" | "viability_evaluator" | "state_verifier" | "telemetry_evaluator" | "wasm_evaluator" => {
            r#"{
  "passed": true,
  "status": "passed|failed",
  "summary": "...",
  "failure_reason": "",
  "evaluator_role": "reviewer|viability_evaluator|state_verifier|telemetry_evaluator|wasm_evaluator",
  "provenance_kind": "brain-judged|state-verified|wasm-computed|runtime-measured|datadog-measured",
  "metrics": {"metric_name":{"value":0,"unit":"score","provenance_kind":"...","interpretation":"..."}},
  "decision_basis": {"why":"...","tradeoffs":["..."]},
  "inputs": {"observations":["..."],"telemetry":["..."],"state":["..."]},
  "evidence_scope": [{"surface":"runtime|logs|traces|metrics|state|wasm","query":"...","time_window":"...","result_count":0,"interpretation":"...","zero_result_meaning":"success|failure|neutral","datadog_url":"https://app.datadoghq.com/..."}],
  "evidence_refs": ["..."],
  "reasoning_summary": "..."
}"#
        }
        "selector" => {
            r#"{
  "winning_variant_id": "...",
  "selection_explanation": "...",
  "app_ref": "...",
  "commit_ref": "...",
  "evidence_uri": "...",
  "digest": "...",
  "tradeoffs": ["..."],
  "reasoning_summary": "..."
}"#
        }
        "promoter" => {
            r#"{
  "status": "succeeded|failed",
  "canonical_app_ref": "owner/app@hash",
  "production_tenant": "default",
  "runtime_ref": "temper://tenant/default/app/owner/app@hash",
  "summary": "...",
  "evidence_refs": ["..."],
  "digest": "...",
  "reasoning_summary": "..."
}"#
        }
        _ => {
            r#"{
  "status": "succeeded|failed",
  "role": "...",
  "work_item_id": "...",
  "evidence_refs": ["..."],
  "reasoning_summary": "...",
  "next_actions": ["..."]
}"#
        }
    }
}

fn literal_prompt_ref(prompt_ref: &str) -> String {
    prompt_ref
        .strip_prefix("literal:")
        .unwrap_or(prompt_ref)
        .trim()
        .to_string()
}

fn directed_evolution_worker_prompt_body(role: &str, prompt_body: &str) -> String {
    let public_api_url = directed_evolution_public_api_url();
    directed_evolution_worker_prompt_body_with_public_api_url(
        role,
        prompt_body,
        public_api_url.as_deref(),
    )
}

fn directed_evolution_worker_prompt_body_with_public_api_url(
    role: &str,
    prompt_body: &str,
    public_api_url: Option<&str>,
) -> String {
    if !matches!(
        role,
        "reviewer"
            | "simulated_user"
            | "viability_evaluator"
            | "state_verifier"
            | "telemetry_evaluator"
            | "wasm_evaluator"
    ) {
        return prompt_body.to_string();
    }

    let mut body = prompt_body.to_string();
    if let Some(public_api_url) = public_api_url {
        body = rewrite_temper_api_base(&body, public_api_url);
    }
    body.push_str(
        "\n\nRuntime execution discipline:\n\
- Prefer the TemperApiBase URL above with the tenant parsed from RuntimeRef; do not assume localhost is the target runtime.\n\
- Do not start a foreground long-lived server. If a local server is absolutely required, start it in the background, stop it before returning, and include the cleanup in evidence_scope.\n\
- If runtime execution is unavailable, fail the stage with clear evidence instead of hanging.\n\
- Simulated-user roles must only report observations and must not include passed, failed, viable, score, winner, or selection fields.\n\
- Evaluator roles may pass/fail only from recorded observations, deterministic checks, runtime measurements, or telemetry evidence with explicit provenance_kind.\n",
    );
    body
}

fn directed_evolution_public_api_url() -> Option<String> {
    env::var("DIRECTED_EVOLUTION_PUBLIC_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(directed_evolution_genesis_url)
}

fn rewrite_temper_api_base(prompt_body: &str, public_api_url: &str) -> String {
    prompt_body
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("TemperApiBase:")
                && (line.contains("127.0.0.1") || line.contains("localhost"))
            {
                format!("TemperApiBase: {}", public_api_url.trim_end_matches('/'))
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn directed_evolution_summary(
    work_item: &DirectedEvolutionWorkItemState,
    output_json: &str,
) -> String {
    let output_summary = serde_json::from_str::<Value>(output_json)
        .ok()
        .and_then(|value| {
            value
                .get("summary")
                .or_else(|| value.get("reasoning_summary"))
                .or_else(|| value.get("selection_explanation"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    if let Some(summary) = output_summary {
        return format!(
            "Directed Evolution {} WorkItem {} completed: {}",
            work_item.role,
            work_item.id,
            truncate_middle(&summary, 600)
        );
    }
    format!(
        "Directed Evolution {} WorkItem {} completed for {}:{} ({} bytes output).",
        work_item.role,
        work_item.id,
        work_item.target_entity_type,
        work_item.target_entity_id,
        output_json.len()
    )
}
