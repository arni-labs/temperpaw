// Native scheduling of agent investigations; provider writes stay on resources.
const DSF_MODEL_RESULT_BEGIN: &str = "DSF_MODEL_RESULT_BEGIN";
const DSF_MODEL_RESULT_END: &str = "DSF_MODEL_RESULT_END";

fn dsf_field<'a>(row: &'a Value, name: &str) -> Option<&'a Value> {
    let pascal = name
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<String>();
    row.get(name).or_else(|| row.get(&pascal)).or_else(|| {
        row.get("fields")
            .and_then(|f| f.get(name).or_else(|| f.get(&pascal)))
    })
}
fn dsf_text(row: &Value, name: &str) -> String {
    dsf_field(row, name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
fn dsf_id(row: &Value) -> String {
    let id = dsf_text(row, "id");
    if id.is_empty() {
        dsf_text(row, "entity_id")
    } else {
        id
    }
}
fn dsf_enabled() -> bool {
    env::var("PAW_DSF_MODEL_PATROL").is_ok_and(|value| value == "1")
}

fn validate_dsf_worker_binding(config: &Config) -> Result<()> {
    if !dsf_enabled() {
        return Ok(());
    }
    if !config.enable_execution {
        bail!("DSF model patrol requires subscription execution enabled");
    }
    let mcp_bin = required_env("DSF_TEMPER_MCP_BIN")?;
    if !Path::new(&mcp_bin).is_file() {
        bail!("DSF_TEMPER_MCP_BIN is not an installed binary");
    }
    if required_env("DSF_FACTORY_AGENT_TOKEN")?.trim().is_empty() {
        bail!("DSF_FACTORY_AGENT_TOKEN is empty");
    }
    if config
        .worker_token
        .as_ref()
        .is_none_or(|token| token.is_empty())
    {
        bail!("DSF model patrol requires its registered WORKER_TOKEN");
    }
    dsf_codex_args(&config.workspace_root, &mcp_bin, &config.temper_url, "")?;
    Ok(())
}

fn dsf_material(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "observation_id"
                            | "source_event_id"
                            | "observed_at_ms"
                            | "window_start"
                            | "window_end"
                            | "createdAt"
                            | "updatedAt"
                            | "created_at"
                            | "updated_at"
                            | "age_seconds"
                            | "oldest_unfinished_at"
                            | "numeric_point_count"
                            | "latest_at_ms"
                            | "generated_at"
                            | "collected_at"
                            | "snapshot_at"
                    )
                })
                .map(|(key, value)| {
                    let material = if key == "latest_point" {
                        value
                            .as_array()
                            .and_then(|v| v.get(1))
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        dsf_material(value)
                    };
                    (key.clone(), material)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(dsf_material).collect()),
        _ => value.clone(),
    }
}
fn dsf_json_field(row: &Value, name: &str) -> Result<Value> {
    match dsf_field(row, name) {
        Some(Value::String(text)) if !text.is_empty() => {
            serde_json::from_str(text).with_context(|| format!("invalid model {name} JSON"))
        }
        Some(value) => Ok(value.clone()),
        None => Ok(Value::Null),
    }
}
fn dsf_investigation_key(source: &Value, observation: &Value) -> Result<String> {
    use sha2::{Digest, Sha256};
    let subject = dsf_text(observation, "subject_type");
    let id = dsf_text(observation, "subject_id");
    if subject.is_empty() || id.is_empty() {
        bail!("observation has no subject");
    }
    let material = json!({"subject_type":subject,"subject_id":id,"source":dsf_text(observation,"source"),"coverage":dsf_text(observation,"status"),"outcome":dsf_text(observation,"outcome"),"intended_configuration":dsf_material(&dsf_json_field(source,"intended_configuration")?),"intended_revision":dsf_text(source,"intended_revision"),"observed_revision":dsf_text(observation,"observed_revision"),"observed_configuration":dsf_material(&dsf_json_field(observation,"observed_configuration")?),"facts":dsf_material(&dsf_json_field(observation,"summary")?)});
    Ok(format!(
        "dsf-model-{:x}",
        Sha256::digest(serde_json::to_vec(&material)?)
    ))
}
async fn dsf_get(
    client: &reqwest::Client,
    config: &Config,
    set: &str,
    id: &str,
) -> Result<Option<Value>> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
    {
        bail!("invalid model entity ID");
    }
    let response = client
        .get(config.entity_url(set, id))
        .headers(headers(config)?)
        .send()
        .await
        .context("read model evidence")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!("read {set} returned {}", response.status());
    }
    let bytes = response.bytes().await?;
    if bytes.len() > 262_144 {
        bail!("model evidence exceeds bound");
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}
async fn reconcile_dsf_model_sources(client: &reqwest::Client, config: &Config) -> Result<()> {
    if !dsf_enabled() {
        return Ok(());
    }
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../os-apps/dsf-twin/specs/module-contracts.json"
    ))?;
    let mut sets = manifest["resources"]
        .as_object()
        .context("DSF resource manifest")?
        .values()
        .map(|r| r["entity_set"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    sets.push("DsfModelSyncs".into());
    for set in sets {
        for page in 0..1000 {
            let response = client
                .get(format!(
                    "{}/tdata/{set}?$orderby=Id&$top=100&$skip={}",
                    config.temper_url,
                    page * 100
                ))
                .headers(headers(config)?)
                .send()
                .await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                bail!("DSF model patrol enabled but {set} is not installed");
            }
            if !response.status().is_success() {
                bail!("DSF source scan {set} returned {}", response.status());
            }
            let rows: Value = response.json().await?;
            let rows = rows["value"].as_array().context("DSF source collection")?;
            for source in rows {
                let status = dsf_text(source, "status");
                if (set != "DsfModelSyncs" && !matches!(status.as_str(), "Active" | "Refreshing"))
                    || (set == "DsfModelSyncs" && matches!(status.as_str(), "Draft" | "Paused"))
                {
                    continue;
                }
                let mut observation_id = dsf_text(source, "collected_observation_id");
                if observation_id.is_empty() {
                    observation_id = dsf_text(source, "observation_id");
                }
                if observation_id.is_empty() {
                    continue;
                }
                if let Some(observation) =
                    dsf_get(client, config, "DsfObservations", &observation_id).await?
                    && matches!(
                        dsf_text(&observation, "status").as_str(),
                        "Measured" | "Absent" | "Inaccessible" | "Stale"
                    )
                {
                    queue_dsf_investigation(client, config, source, &observation_id, &observation)
                        .await?;
                }
            }
            if rows.len() < 100 {
                break;
            }
            if page == 999 {
                bail!("DSF source scan exceeded 100000 rows in {set}");
            }
        }
    }
    Ok(())
}
async fn queue_dsf_investigation(
    client: &reqwest::Client,
    config: &Config,
    source: &Value,
    observation_id: &str,
    observation: &Value,
) -> Result<()> {
    let key = dsf_investigation_key(source, observation)?;
    let source_evidence=json!({"observation_id":observation_id,"observation":observation,"intended_configuration":dsf_field(source,"intended_configuration"),"intended_revision":dsf_field(source,"intended_revision"),"source_entity_id":dsf_id(source)}).to_string();
    if source_evidence.len() > 65536 {
        bail!("investigation provenance exceeds 64KiB");
    }
    if let Some(existing) = dsf_get(client, config, "PatrolRuns", &key).await? {
        if dsf_text(&existing, "status") != "Created" {
            let status = dsf_text(&existing, "status");
            if matches!(status.as_str(), "Queued" | "Running") {
                let worker_id = dsf_text(&existing, "worker_run_id");
                match dsf_get(client, config, "WorkerRuns", &worker_id).await? {
                    None if status == "Queued" => {
                        post_entity_action(
                            client,
                            config,
                            "PatrolRuns",
                            &key,
                            "ReconcileModelWorker",
                            json!({}),
                        )
                        .await?
                    }
                    Some(worker)
                        if status == "Queued"
                            && dsf_text(&worker, "patrol_run_id").is_empty()
                            && dsf_text(&worker, "status") == "Queued" =>
                    {
                        post_entity_action(
                            client,
                            config,
                            "PatrolRuns",
                            &key,
                            "ReconcileModelWorker",
                            json!({}),
                        )
                        .await?
                    }
                    Some(worker)
                        if dsf_text(&worker, "status") == "Claimed"
                            && dsf_text(&worker, "worker_id") == config.worker_id =>
                    {
                        start_local_worker_run(client, config, &worker_id).await?;
                        handle_running_worker_run(client, config, &worker_id).await?;
                    }
                    Some(worker) if dsf_text(&worker, "status") == "Done" => {
                        post_action(
                            client,
                            config,
                            &worker_id,
                            "ReplayInvestigationResult",
                            json!({}),
                        )
                        .await?
                    }
                    Some(worker)
                        if matches!(
                            dsf_text(&worker, "status").as_str(),
                            "Failed" | "TimedOut"
                        ) =>
                    {
                        post_action(
                            client,
                            config,
                            &worker_id,
                            "ReplayInvestigationFailure",
                            json!({}),
                        )
                        .await?
                    }
                    _ => {}
                }
            }
            return Ok(());
        }
    } else {
        let response = client
            .post(format!("{}/tdata/PatrolRuns", config.temper_url))
            .headers(headers(config)?)
            .json(&json!({"id":key}))
            .send()
            .await?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::CONFLICT {
            bail!("create model PatrolRun returned {}", response.status());
        }
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Ok(());
        }
    }
    let worker_id = format!("dsf-worker-{}", key.trim_start_matches("dsf-model-"));
    let branch = format!("codex/{key}");
    let task = json!({"kind":"dsf_model_investigation","patrol_run_id":key}).to_string();
    post_entity_action(client,config,"PatrolRuns",&key,"RequestModelInvestigation",json!({"investigation_key":key,"observation_id":observation_id,"source_evidence":source_evidence,"worker_run_id":worker_id,"task":task,"branch_name":branch,"worktree_path":config.workspace_root.join(&key).to_string_lossy(),"allowed_worker_id":config.worker_id,"provider_id":"local-codex","runner_kind":"local_codex","requested_by":config.worker_id})).await
}
fn dsf_patrol_id(task: &str) -> Option<String> {
    let value: Value = serde_json::from_str(task).ok()?;
    (value["kind"] == "dsf_model_investigation")
        .then(|| value["patrol_run_id"].as_str().map(str::to_owned))
        .flatten()
}
