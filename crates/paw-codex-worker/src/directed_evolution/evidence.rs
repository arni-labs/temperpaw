#[allow(clippy::too_many_arguments)]
async fn record_directed_evolution_worker_evidence(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    artifact_kind: &str,
    output_json: &str,
    summary: &str,
    observe_metadata: Option<&str>,
) -> Result<String> {
    let output_value = serde_json::from_str::<Value>(output_json).unwrap_or_else(|_| {
        json!({
            "raw": output_json,
        })
    });
    let evidence_id = create_entity_with_observe_metadata(
        client,
        config,
        "EvidenceArtifacts",
        json!({}),
        observe_metadata,
    )
    .await?;
    let uri = directed_evolution_evidence_uri(work_item, &output_value);
    let correlation =
        directed_evolution_evidence_correlation(work_item, worker_run_id, output_value.clone());
    let evidence_summary = directed_evolution_first_evidence_scope_summary(&output_value);
    post_directed_evolution_action(
        client,
        config,
        "EvidenceArtifacts",
        &evidence_id,
        "RecordEvidenceSummary",
        json!({
            "ArtifactKind": artifact_kind,
            "Uri": uri,
            "Summary": summary,
            "CorrelationJson": correlation.to_string(),
            "Digest": directed_evolution_evidence_digest(output_json),
            "Query": evidence_summary.query,
            "TimeWindow": evidence_summary.time_window,
            "ResultCount": evidence_summary.result_count,
            "Interpretation": evidence_summary.interpretation,
            "ZeroResultMeaning": evidence_summary.zero_result_meaning,
            "EvidenceProvenance": directed_evolution_evidence_provenance(&work_item.role, &output_value),
        }),
        observe_metadata,
    )
    .await?;
    post_directed_evolution_action(
        client,
        config,
        "EvidenceArtifacts",
        &evidence_id,
        "LinkEvidenceArtifact",
        directed_evolution_evidence_link_body(worker_run_id),
        observe_metadata,
    )
    .await?;
    Ok(evidence_id)
}

fn directed_evolution_evidence_correlation(
    work_item: &DirectedEvolutionWorkItemState,
    worker_run_id: &str,
    output_value: Value,
) -> Value {
    json!({
        "work_item_id": work_item.id,
        "worker_run_id": worker_run_id,
        "role": work_item.role,
        "target_entity_type": work_item.target_entity_type,
        "target_entity_id": work_item.target_entity_id,
        "context_ref": work_item.context_ref,
        "output_schema_ref": work_item.output_schema_ref,
        "datadog": directed_evolution_datadog_context(work_item),
        "output": output_value,
    })
}

fn directed_evolution_evidence_link_body(worker_run_id: &str) -> Value {
    json!({
        "TargetEntityType": "WorkerRun",
        "TargetEntityId": worker_run_id,
    })
}

#[derive(Default)]
struct DirectedEvolutionEvidenceScopeSummary {
    query: String,
    time_window: String,
    result_count: String,
    interpretation: String,
    zero_result_meaning: String,
}

fn directed_evolution_first_evidence_scope_summary(
    output: &Value,
) -> DirectedEvolutionEvidenceScopeSummary {
    let Some(items) = output
        .get("evidence_scope")
        .or_else(|| output.get("evidenceScope"))
        .and_then(Value::as_array)
    else {
        return DirectedEvolutionEvidenceScopeSummary::default();
    };
    let Some(first) = items.first() else {
        return DirectedEvolutionEvidenceScopeSummary::default();
    };
    DirectedEvolutionEvidenceScopeSummary {
        query: value_field_string(first, &["query", "Query"]),
        time_window: value_field_string(first, &["time_window", "timeWindow", "TimeWindow"]),
        result_count: value_field_string(first, &["result_count", "resultCount", "count"]),
        interpretation: value_field_string(
            first,
            &[
                "interpretation",
                "Interpretation",
                "result_summary",
                "resultSummary",
            ],
        ),
        zero_result_meaning: value_field_string(
            first,
            &[
                "zero_result_meaning",
                "zeroResultMeaning",
                "ZeroResultMeaning",
            ],
        ),
    }
}

fn directed_evolution_evidence_provenance(role: &str, output: &Value) -> String {
    let fallback = match role {
        "simulated_user" => "agent-observed",
        "state_verifier" => "state-verified",
        "telemetry_evaluator" => "datadog-measured",
        "wasm_evaluator" => "wasm-computed",
        "reviewer" | "viability_evaluator" => "brain-judged",
        _ => "agent-observed",
    };
    nonempty(
        value_field_string(
            output,
            &[
                "provenance_kind",
                "provenanceKind",
                "EvidenceProvenance",
                "evidence_provenance",
            ],
        ),
        fallback.to_string(),
    )
}

fn directed_evolution_evidence_uri(
    work_item: &DirectedEvolutionWorkItemState,
    output: &Value,
) -> String {
    if let Some(url) = first_datadog_evidence_url(output) {
        return url;
    }
    for key in [
        "evidence_uri",
        "evidenceRef",
        "evidence_ref",
        "diff_ref",
        "diffRef",
        "runtime_ref",
        "runtimeRef",
    ] {
        if let Some(value) = output.get(key).and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return value.trim().to_string();
        }
    }
    if let Some(first) = output
        .get("evidence_refs")
        .or_else(|| output.get("evidenceRefs"))
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find_map(Value::as_str))
        .filter(|value| !value.trim().is_empty())
    {
        return first.trim().to_string();
    }
    format!(
        "temperpaw://directed-evolution/{}/{}",
        sanitize_path_component(&work_item.role),
        sanitize_path_component(&work_item.id)
    )
}

fn first_datadog_evidence_url(output: &Value) -> Option<String> {
    for key in ["evidence_scope", "evidenceScope"] {
        let Some(items) = output.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let Some(url) = item
                .get("datadog_url")
                .or_else(|| item.get("datadogUrl"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            if is_datadog_app_url(url) {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn is_datadog_app_url(url: &str) -> bool {
    [
        "https://app.datadoghq.com",
        "https://app.us3.datadoghq.com",
        "https://app.us5.datadoghq.com",
        "https://app.datadoghq.eu",
        "https://app.ap1.datadoghq.com",
        "https://app.ap2.datadoghq.com",
        "https://app.ddog-gov.com",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix))
}

fn directed_evolution_evidence_digest(output_json: &str) -> String {
    format!("bytes:{}", output_json.len())
}

fn directed_evolution_datadog_context(work_item: &DirectedEvolutionWorkItemState) -> Value {
    let service = env::var("DD_SERVICE").unwrap_or_else(|_| "temperpaw".to_string());
    let env_name = env::var("DD_ENV").unwrap_or_else(|_| "local".to_string());
    let site = env::var("DD_SITE").unwrap_or_else(|_| "datadoghq.com".to_string());
    let query = format!("service:{service} env:{env_name} @work_item_id:{}", work_item.id);
    let mut context = json!({
        "service": service,
        "env": env_name,
        "work_item_id": work_item.id,
        "role": work_item.role,
        "target_entity_type": work_item.target_entity_type,
        "target_entity_id": work_item.target_entity_id,
        "control_tenant": config_tenant_label(),
        "query": query,
        "logs_url": format!(
            "https://app.{site}/logs?query={}",
            encode_url_component(&query)
        ),
    });
    let join = directed_evolution_join_fields(&work_item.correlation_json);
    if let Some(object) = context.as_object_mut() {
        for (key, value) in join.entries() {
            object.insert(key.to_string(), json!(value));
        }
    }
    context
}

fn config_tenant_label() -> String {
    env::var("TEMPER_TENANT").unwrap_or_else(|_| "default".to_string())
}

fn encode_url_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

struct DirectedEvolutionDatadogEventSearch<'a> {
    site: &'a str,
    path: &'a str,
    api_key: &'a str,
    app_key: &'a str,
    query: &'a str,
    from: &'a str,
    to: &'a str,
}

async fn directed_evolution_datadog_event_search(
    client: &reqwest::Client,
    request: DirectedEvolutionDatadogEventSearch<'_>,
) -> Result<Value> {
    let url = format!("https://api.{}{}", request.site, request.path);
    let request_body = if request.path.contains("/spans/") {
        json!({
            "data": {
                "type": "search_request",
                "attributes": {
                    "filter": {
                        "query": request.query,
                        "from": request.from,
                        "to": request.to,
                    },
                    "options": {
                        "timezone": "GMT",
                    },
                    "page": {
                        "limit": 25,
                    },
                    "sort": "-timestamp",
                },
            },
        })
    } else {
        json!({
            "filter": {
                "query": request.query,
                "from": request.from,
                "to": request.to,
            },
            "page": {
                "limit": 25,
            },
        })
    };
    let response = client
        .post(url)
        .header("DD-API-KEY", request.api_key)
        .header("DD-APPLICATION-KEY", request.app_key)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .json(&request_body)
        .send()
        .await
        .with_context(|| format!("query Datadog events for {}", request.query))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(json!({
            "ok": false,
            "http_status": status.to_string(),
            "body_preview": truncate_middle(&text, 1_200),
        }));
    }
    let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
        json!({
            "raw": truncate_middle(&text, 1_200),
        })
    });
    Ok(directed_evolution_datadog_event_summary(&value))
}

fn directed_evolution_datadog_event_summary(value: &Value) -> Value {
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let samples = items
        .iter()
        .take(8)
        .map(directed_evolution_datadog_event_sample)
        .collect::<Vec<_>>();
    json!({
        "ok": true,
        "result_count": items.len(),
        "samples": samples,
    })
}

fn directed_evolution_datadog_event_sample(item: &Value) -> Value {
    let attributes = item.get("attributes").unwrap_or(item);
    let nested = attributes.get("attributes").unwrap_or(attributes);
    json!({
        "id": value_field_string(item, &["id"]),
        "timestamp": value_field_string(attributes, &["timestamp"]),
        "service": value_field_string(nested, &["service"]),
        "message": truncate_middle(
            &value_field_string(attributes, &["message", "resource_name", "name"]),
            300
        ),
        "action": value_field_string(nested, &["action", "temper.action"]),
        "entity_type": value_field_string(nested, &["entity_type", "temper.entity_type"]),
        "entity_id": value_field_string(nested, &["entity_id", "temper.entity_id"]),
        "observation_metadata": truncate_middle(
            &value_field_string(nested, &["observation_metadata", "temper.observation_metadata"]),
            500
        ),
    })
}
