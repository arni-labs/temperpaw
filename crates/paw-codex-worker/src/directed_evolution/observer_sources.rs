async fn directed_evolution_observer_source_inventory_prompt(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    workdir: &DirectedEvolutionWorkdir,
    observe_metadata: &str,
) -> Result<String> {
    let correlation = serde_json::from_str::<Value>(&work_item.correlation_json)
        .unwrap_or_else(|_| json!({}));
    let inventory = json!({
        "inventory_contract": {
            "purpose": "Describe available observation sources before the observer infers pressures.",
            "not_a_script": true,
            "observer_instruction": "Use these discovered sources as a starting map. Query or inspect additional accessible sources when useful. Do not conclude from one source when stronger or contradictory sources are available.",
            "required_source_classes": [
                "genesis_control_plane",
                "runtime_odata_state",
                "datadog_observability",
                "trajectory_or_unmet_intent",
                "app_source_or_description"
            ]
        },
        "target": {
            "work_item_id": work_item.id,
            "target_entity_type": work_item.target_entity_type,
            "target_entity_id": work_item.target_entity_id,
            "context_ref": work_item.context_ref,
            "output_schema_ref": work_item.output_schema_ref,
            "workdir": workdir.path.display().to_string(),
            "app_ref": workdir.app_ref,
            "branch_ref": workdir.branch_ref,
            "canonical_genesis_bundle": workdir.canonical_genesis_bundle,
        },
        "correlation": correlation,
        "sources": {
            "genesis_control_plane": directed_evolution_observer_genesis_sources(client, config, work_item, &correlation).await,
            "runtime_odata_state": directed_evolution_observer_runtime_sources(client, &correlation, observe_metadata).await,
            "datadog_observability": directed_evolution_observer_datadog_sources(client, work_item, &correlation).await,
            "app_source_or_description": directed_evolution_observer_app_sources(workdir, &correlation),
        },
    });
    serde_json::to_string_pretty(&inventory).context("serialize observer source inventory")
}

async fn directed_evolution_observer_genesis_sources(
    client: &reqwest::Client,
    config: &Config,
    work_item: &DirectedEvolutionWorkItemState,
    correlation: &Value,
) -> Value {
    let explicit_work_item_ids =
        observer_string_array(correlation, "simulated_user_work_item_ids");
    let exact_work_items =
        directed_evolution_observer_exact_work_items(client, config, &explicit_work_item_ids).await;
    let collections = [
        "Signals",
        "Pressures",
        "Directions",
        "WorkItems",
        "WorkerRuns",
        "BrainRuns",
        "EvidenceArtifacts",
        "Episodes",
        "Trials",
        "Measurements",
        "Mutations",
    ];
    let mut sampled = serde_json::Map::new();
    for entity_set in collections {
        sampled.insert(
            entity_set.to_string(),
            directed_evolution_observer_collection_sample(client, config, entity_set, work_item)
                .await,
        );
    }
    json!({
        "status": "queried",
        "tenant": config.tenant,
        "exact_simulated_user_work_items": exact_work_items,
        "collections": sampled,
    })
}

async fn directed_evolution_observer_exact_work_items(
    client: &reqwest::Client,
    config: &Config,
    ids: &[String],
) -> Value {
    let mut samples = Vec::new();
    for id in ids.iter().take(8) {
        match fetch_directed_evolution_entity_fields(client, config, "WorkItems", id).await {
            Ok(fields) => {
                let worker_run_id = value_field_string(&fields, &["WorkerRunId", "worker_run_id"]);
                let brain_run_id = value_field_string(&fields, &["BrainRunId", "brain_run_id"]);
                let evidence_artifact_id =
                    value_field_string(&fields, &["EvidenceArtifactId", "evidence_artifact_id"]);
                let worker_run = if !worker_run_id.is_empty() {
                    directed_evolution_observer_fetch_entity_sample(
                        client,
                        config,
                        "WorkerRuns",
                        &worker_run_id,
                    )
                    .await
                } else if !brain_run_id.is_empty() {
                    directed_evolution_observer_fetch_entity_sample(
                        client,
                        config,
                        "BrainRuns",
                        &brain_run_id,
                    )
                    .await
                } else {
                    json!({"status": "not_linked"})
                };
                let evidence_artifact = if evidence_artifact_id.is_empty() {
                    json!({"status": "not_linked"})
                } else {
                    directed_evolution_observer_fetch_entity_sample(
                        client,
                        config,
                        "EvidenceArtifacts",
                        &evidence_artifact_id,
                    )
                    .await
                };
                samples.push(json!({
                    "id": id,
                    "work_item": directed_evolution_observer_fields_sample("WorkItems", id, &fields),
                    "worker_run": worker_run,
                    "evidence_artifact": evidence_artifact,
                }));
            }
            Err(error) => samples.push(json!({
                "id": id,
                "status": "unavailable",
                "error": truncate_middle(&error.to_string(), 500),
            })),
        }
    }
    json!({
        "requested_count": ids.len(),
        "sample_count": samples.len(),
        "samples": samples,
    })
}

async fn directed_evolution_observer_collection_sample(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    work_item: &DirectedEvolutionWorkItemState,
) -> Value {
    match query_directed_evolution_rows(client, config, entity_set, "", 40).await {
        Ok(rows) => {
            let samples = rows
                .iter()
                .filter(|row| directed_evolution_observer_row_relevant(row, work_item))
                .take(5)
                .map(|row| {
                    let fields = directed_evolution_row_fields(row);
                    let id = directed_evolution_row_id(row);
                    directed_evolution_observer_fields_sample(entity_set, &id, &fields)
                })
                .collect::<Vec<_>>();
            json!({
                "status": "available",
                "result_count": rows.len(),
                "relevant_sample_count": samples.len(),
                "samples": samples,
            })
        }
        Err(error) => json!({
            "status": "unavailable",
            "error": truncate_middle(&error.to_string(), 700),
        }),
    }
}

async fn directed_evolution_observer_fetch_entity_sample(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    id: &str,
) -> Value {
    match fetch_directed_evolution_entity_fields(client, config, entity_set, id).await {
        Ok(fields) => directed_evolution_observer_fields_sample(entity_set, id, &fields),
        Err(error) => json!({
            "id": id,
            "entity_set": entity_set,
            "status": "unavailable",
            "error": truncate_middle(&error.to_string(), 500),
        }),
    }
}

fn directed_evolution_observer_row_relevant(
    row: &Value,
    work_item: &DirectedEvolutionWorkItemState,
) -> bool {
    let text = row.to_string();
    [
        work_item.id.as_str(),
        work_item.target_entity_id.as_str(),
        work_item.context_ref.as_str(),
        "simulated_user",
        "observer",
        "unmet",
        "friction",
        "intent",
    ]
    .iter()
    .any(|needle| !needle.trim().is_empty() && text.contains(needle))
}

fn directed_evolution_observer_fields_sample(entity_set: &str, id: &str, fields: &Value) -> Value {
    json!({
        "entity_set": entity_set,
        "id": id,
        "status": value_field_string(fields, &["Status", "status"]),
        "role": value_field_string(fields, &["Role", "role"]),
        "source": value_field_string(fields, &["Source", "source"]),
        "signal_kind": value_field_string(fields, &["SignalKind", "signal_kind"]),
        "target_entity_type": value_field_string(fields, &["TargetEntityType", "target_entity_type"]),
        "target_entity_id": value_field_string(fields, &["TargetEntityId", "target_entity_id"]),
        "worker_run_id": value_field_string(fields, &["WorkerRunId", "worker_run_id", "BrainRunId", "brain_run_id"]),
        "evidence_artifact_id": value_field_string(fields, &["EvidenceArtifactId", "evidence_artifact_id"]),
        "query": truncate_middle(&value_field_string(fields, &["Query", "query"]), 300),
        "time_window": value_field_string(fields, &["TimeWindow", "time_window"]),
        "result_count": value_field_string(fields, &["ResultCount", "result_count"]),
        "interpretation": truncate_middle(&value_field_string(fields, &["Interpretation", "interpretation"]), 500),
        "summary": truncate_middle(&value_field_string(fields, &["Summary", "summary", "FailureReason", "failure_reason"]), 600),
        "result_json": truncate_middle(&value_field_string(fields, &["ResultJson", "result_json", "OutputJson", "output_json"]), 700),
        "correlation_json": truncate_middle(&value_field_string(fields, &["CorrelationJson", "correlation_json"]), 500),
    })
}

async fn directed_evolution_observer_runtime_sources(
    client: &reqwest::Client,
    correlation: &Value,
    observe_metadata: &str,
) -> Value {
    let Some(runtime_base) = observer_string(correlation, "runtime_base_url")
        .or_else(|| observer_string(correlation, "runtimeBaseUrl"))
        .filter(|value| !value.trim().is_empty())
    else {
        return json!({
            "status": "unavailable",
            "reason": "CorrelationJson did not provide runtime_base_url",
        });
    };
    let runtime_base = runtime_base.trim_end_matches('/').to_string();
    let tenant = observer_string(correlation, "runtime_tenant")
        .or_else(|| observer_string(correlation, "runtimeTenant"))
        .unwrap_or_default();
    let auth_token = observer_runtime_auth_token(correlation);
    let metadata = observer_runtime_get_text(
        client,
        &format!("{runtime_base}/tdata/$metadata"),
        &tenant,
        auth_token.as_deref(),
        Some(observe_metadata),
    )
    .await;
    let entity_sets = metadata
        .get("body_preview")
        .and_then(Value::as_str)
        .map(observer_entity_sets_from_metadata)
        .unwrap_or_default();
    let entity_sets_to_sample = observer_runtime_entity_sets_to_sample(&entity_sets);
    let mut samples = Vec::new();
    for entity_set in entity_sets_to_sample {
        samples.push(
            observer_runtime_collection_sample(
                client,
                &runtime_base,
                &tenant,
                auth_token.as_deref(),
                &entity_set,
                Some(observe_metadata),
            )
            .await,
        );
    }
    json!({
        "status": "queried",
        "runtime_base_url": runtime_base,
        "runtime_tenant": tenant,
        "auth": if auth_token.is_some() { "bearer-token-from-env" } else { "none" },
        "metadata": metadata,
        "entity_sets": entity_sets,
        "entity_samples": samples,
    })
}

async fn observer_runtime_get_text(
    client: &reqwest::Client,
    url: &str,
    tenant: &str,
    auth_token: Option<&str>,
    observe_metadata: Option<&str>,
) -> Value {
    let mut request = client.get(url).header(ACCEPT, "application/json");
    if !tenant.trim().is_empty() {
        request = request.header("X-Tenant-Id", tenant);
    }
    if let Some(token) = auth_token.filter(|value| !value.trim().is_empty()) {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(metadata) = observe_metadata.filter(|value| !value.trim().is_empty()) {
        request = request.header(TEMPER_OBSERVE_METADATA_HEADER, metadata);
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            json!({
                "status": if status.is_success() { "available" } else { "unavailable" },
                "http_status": status.to_string(),
                "body_preview": truncate_middle(&text, 1_200),
            })
        }
        Err(error) => json!({
            "status": "unavailable",
            "error": truncate_middle(&error.to_string(), 700),
        }),
    }
}

async fn observer_runtime_collection_sample(
    client: &reqwest::Client,
    runtime_base: &str,
    tenant: &str,
    auth_token: Option<&str>,
    entity_set: &str,
    observe_metadata: Option<&str>,
) -> Value {
    let url = format!("{runtime_base}/tdata/{entity_set}?$top=12");
    let result =
        observer_runtime_get_text(client, &url, tenant, auth_token, observe_metadata).await;
    let samples = result
        .get("body_preview")
        .and_then(Value::as_str)
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .and_then(|body| body.get("value").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .take(4)
        .map(observer_runtime_row_sample)
        .collect::<Vec<_>>();
    json!({
        "entity_set": entity_set,
        "query": format!("GET /tdata/{entity_set}?$top=12"),
        "result": result,
        "samples": samples,
    })
}

fn observer_runtime_row_sample(row: &Value) -> Value {
    let fields = row.get("fields").unwrap_or(row);
    json!({
        "id": first_string(row, fields, &["entity_id", "id", "Id"], &["Id", "id"]),
        "status": value_field_string(fields, &["Status", "status"]),
        "question_id": value_field_string(fields, &["QuestionId", "question_id"]),
        "answer_id": value_field_string(fields, &["AnswerId", "answer_id"]),
        "title": truncate_middle(&value_field_string(fields, &["Title", "title", "Question", "question"]), 300),
        "content": truncate_middle(&value_field_string(fields, &["Content", "content", "Answer", "answer"]), 400),
        "raw": truncate_middle(&row.to_string(), 500),
    })
}

async fn directed_evolution_observer_datadog_sources(
    client: &reqwest::Client,
    work_item: &DirectedEvolutionWorkItemState,
    correlation: &Value,
) -> Value {
    let Some(api_key) = env::var("DD_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return json!({
            "status": "unavailable",
            "reason": "DD_API_KEY is not present in the worker environment",
        });
    };
    let Some(app_key) = env::var("DD_APP_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return json!({
            "status": "unavailable",
            "reason": "DD_APP_KEY is not present in the worker environment",
        });
    };
    let site = env::var("DD_SITE")
        .ok()
        .map(|value| value.trim().trim_start_matches("app.").to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "datadoghq.com".to_string());
    let service = observer_string(correlation, "runtime_datadog_service")
        .or_else(|| env::var("DD_SERVICE").ok())
        .unwrap_or_else(|| "temperpaw".to_string());
    let batch_id = observer_string(correlation, "batch_id").unwrap_or_else(|| work_item.id.clone());
    let runtime_tenant = observer_string(correlation, "runtime_tenant").unwrap_or_default();
    let app_ref = observer_string(correlation, "app_ref").unwrap_or_default();
    let from = observer_string(correlation, "datadog_from").unwrap_or_else(|| "now-4h".to_string());
    let to = observer_string(correlation, "datadog_to").unwrap_or_else(|| "now".to_string());

    let mut log_queries = vec![
        ("app_usage_recent", format!("service:{service} \"app usage:\"")),
        (
            "trajectory_entries",
            format!("service:{service} \"trajectory.entry\""),
        ),
        (
            "unmet_intent_terms",
            format!("service:{service} (\"unmet\" OR \"intent_satisfied\" OR \"blocker\" OR \"friction\")"),
        ),
    ];
    if !batch_id.trim().is_empty() {
        log_queries.push(("batch_correlation", format!("service:{service} \"{batch_id}\"")));
    }
    if !runtime_tenant.trim().is_empty() {
        log_queries.push(("runtime_tenant", format!("service:{service} \"{runtime_tenant}\"")));
    }
    if !app_ref.trim().is_empty() {
        log_queries.push(("app_ref", format!("service:{service} \"{app_ref}\"")));
    }
    let mut logs = Vec::new();
    for (label, query) in log_queries {
        let result = directed_evolution_datadog_event_search(
            client,
            DirectedEvolutionDatadogEventSearch {
                site: &site,
                path: "/api/v2/logs/events/search",
                api_key: &api_key,
                app_key: &app_key,
                query: &query,
                from: &from,
                to: &to,
            },
        )
        .await
        .unwrap_or_else(|error| {
            json!({
                "ok": false,
                "error": truncate_middle(&error.to_string(), 700),
            })
        });
        let datadog_url = observer_datadog_logs_url(&site, &query, &from, &to);
        logs.push(json!({
            "label": label,
            "query": query,
            "datadog_url": datadog_url,
            "result": result,
        }));
    }

    let mut trace_queries = vec![("service_recent", format!("service:{service}"))];
    if !batch_id.trim().is_empty() {
        trace_queries.push((
            "batch_observation_metadata",
            format!("service:{service} @observation_metadata:*{batch_id}*"),
        ));
    }
    let mut traces = Vec::new();
    for (label, query) in trace_queries {
        let result = directed_evolution_datadog_event_search(
            client,
            DirectedEvolutionDatadogEventSearch {
                site: &site,
                path: "/api/v2/spans/events/search",
                api_key: &api_key,
                app_key: &app_key,
                query: &query,
                from: &from,
                to: &to,
            },
        )
        .await
        .unwrap_or_else(|error| {
            json!({
                "ok": false,
                "error": truncate_middle(&error.to_string(), 700),
            })
        });
        traces.push(json!({
            "label": label,
            "query": query,
            "datadog_url": observer_datadog_traces_url(&site, &query, &from, &to),
            "result": result,
        }));
    }

    json!({
        "status": "queried",
        "site": site,
        "service": service,
        "time_window": format!("{from}..{to}"),
        "logs": logs,
        "traces": traces,
        "metrics_and_monitors": {
            "status": "available_to_observer",
            "note": "The observer Codex process inherits DD_API_KEY/DD_APP_KEY/DD_SITE and may query metrics, monitors, dashboards, or incidents when logs/traces/state suggest they are relevant."
        }
    })
}

fn directed_evolution_observer_app_sources(
    workdir: &DirectedEvolutionWorkdir,
    correlation: &Value,
) -> Value {
    let files = observer_collect_app_source_files(&workdir.path, correlation);
    json!({
        "status": if files.is_empty() { "no_candidate_files_found" } else { "available" },
        "source_kind": if workdir.canonical_genesis_bundle { "canonical_genesis_bundle" } else { "workspace_source_candidates" },
        "warning": if workdir.canonical_genesis_bundle { "" } else { "These are local workspace candidates. Treat them as orientation only unless their path/content clearly matches the target app." },
        "workdir": workdir.path.display().to_string(),
        "files": files,
    })
}

fn observer_collect_app_source_files(root: &Path, correlation: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    observer_collect_app_source_files_inner(root, root, correlation, 0, &mut found);
    found.truncate(5);
    found
}

fn observer_collect_app_source_files_inner(
    root: &Path,
    dir: &Path,
    correlation: &Value,
    depth: usize,
    found: &mut Vec<Value>,
) {
    if depth > 5 || found.len() >= 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if observer_skip_source_path(&name) {
            continue;
        }
        if path.is_dir() {
            observer_collect_app_source_files_inner(root, &path, correlation, depth + 1, found);
            continue;
        }
        if observer_source_file_matches(&path, correlation)
            && let Ok(contents) = fs::read_to_string(&path)
        {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            found.push(json!({
                "path": relative,
                "bytes": contents.len(),
                "preview": truncate_middle(&contents, 800),
            }));
        }
    }
}

fn observer_source_file_matches(path: &Path, correlation: &Value) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "app.md" | "readme.md" | "package.json" | "csdl.xml" | "metadata.xml"
    ) {
        return true;
    }
    if name.ends_with(".ioa.toml") {
        return true;
    }
    let app_id = observer_string(correlation, "app_id")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "_");
    let app_ref = observer_string(correlation, "app_ref")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace(['/', '@', '-'], "_");
    !app_id.is_empty() && name.contains(&app_id) || !app_ref.is_empty() && name.contains(&app_ref)
}

fn observer_skip_source_path(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".svelte-kit" | "dist" | "build" | ".next"
    )
}

fn observer_runtime_auth_token(correlation: &Value) -> Option<String> {
    observer_string_array(correlation, "runtime_auth_env_vars")
        .into_iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .or_else(|| {
            ["TEMPERPAW_RUNTIME_API_KEY", "TEMPER_API_KEY"]
                .into_iter()
                .filter_map(|key| env::var(key).ok())
                .map(|value| value.trim().to_string())
                .find(|value| !value.is_empty())
        })
}

fn observer_entity_sets_from_metadata(metadata: &str) -> Vec<String> {
    let mut names = Vec::new();
    for part in metadata.split("EntitySet Name=\"").skip(1) {
        let Some(name) = part.split('"').next() else {
            continue;
        };
        if !name.trim().is_empty() && !names.iter().any(|item| item == name) {
            names.push(name.to_string());
        }
    }
    names
}

fn observer_runtime_entity_sets_to_sample(entity_sets: &[String]) -> Vec<String> {
    let preferred = [
        "Questions",
        "Answers",
        "Users",
        "Sessions",
        "Signals",
        "IntentDiscoveries",
        "Trajectories",
    ];
    let mut selected = Vec::new();
    for name in preferred {
        if entity_sets.iter().any(|item| item == name) {
            selected.push(name.to_string());
        }
    }
    for name in entity_sets {
        if selected.len() >= 8 {
            break;
        }
        if !selected.iter().any(|item| item == name) {
            selected.push(name.clone());
        }
    }
    selected
}

fn observer_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn observer_string_array(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn observer_datadog_logs_url(site: &str, query: &str, from: &str, to: &str) -> String {
    format!(
        "https://app.{site}/logs?query={}&from_ts={}&to_ts={}&live=false",
        encode_url_component(query),
        encode_url_component(from),
        encode_url_component(to)
    )
}

fn observer_datadog_traces_url(site: &str, query: &str, from: &str, to: &str) -> String {
    format!(
        "https://app.{site}/apm/traces?query={}&from_ts={}&to_ts={}&live=false",
        encode_url_component(query),
        encode_url_component(from),
        encode_url_component(to)
    )
}
