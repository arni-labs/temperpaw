async fn claim_boot_queued_runs(client: &reqwest::Client, config: &Config) -> Result<()> {
    let runs = query_boot_worker_runs(client, config, "Queued").await?;
    for worker_run in runs {
        if worker_run.status == "Queued" {
            handle_queued_worker_run(client, config, &worker_run.id).await?;
        }
    }
    Ok(())
}

async fn recover_boot_running_runs(client: &reqwest::Client, config: &Config) -> Result<()> {
    let runs = query_boot_worker_runs(client, config, "Running").await?;
    for worker_run in runs {
        if worker_run_is_recoverable_by_local_codex(&worker_run, &config.worker_id) {
            handle_running_worker_run(client, config, &worker_run.id).await?;
        }
    }
    Ok(())
}

async fn query_boot_worker_runs(
    client: &reqwest::Client,
    config: &Config,
    status: &str,
) -> Result<Vec<WorkerRunState>> {
    let url = format!(
        "{}/tdata/WorkerRuns?$filter=Status eq '{}'&$orderby=Id desc&$top=50",
        config.temper_url, status
    );
    let response = client
        .get(url)
        .headers(headers(config)?)
        .send()
        .await
        .with_context(|| format!("query {status} WorkerRuns on boot"))?;
    if !response.status().is_success() {
        warn!(http_status = %response.status(), status, "WorkerRun boot query failed");
        return Ok(Vec::new());
    }

    let body: Value = response
        .json()
        .await
        .with_context(|| format!("parse {status} WorkerRun response"))?;
    let Some(runs) = body
        .get("value")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    runs.iter()
        .map(|run| worker_run_from_odata_value(run.clone()))
        .collect()
}

async fn claim_boot_requested_review_runs(client: &reqwest::Client, config: &Config) -> Result<()> {
    let ids = query_boot_entity_ids(client, config, "ReviewRuns", "Requested").await?;
    for review_run_id in ids {
        handle_requested_review_run(client, config, &review_run_id).await?;
    }
    Ok(())
}

async fn claim_boot_queued_evaluation_runs(
    client: &reqwest::Client,
    config: &Config,
) -> Result<()> {
    let ids = query_boot_entity_ids(client, config, "EvaluationRuns", "Queued").await?;
    for evaluation_run_id in ids {
        handle_queued_evaluation_run(client, config, &evaluation_run_id).await?;
    }
    Ok(())
}

async fn claim_boot_queued_directed_evolution_work_items(
    client: &reqwest::Client,
    config: &Config,
) -> Result<()> {
    let ids = query_boot_entity_ids(client, config, "WorkItems", "Queued").await?;
    if ids.is_empty() {
        return Ok(());
    }

    let concurrency = config.max_concurrent_runs.max(1);
    info!(
        queued_work_item_count = ids.len(),
        max_concurrent_runs = concurrency,
        "claiming queued Directed Evolution WorkItems"
    );
    let results = futures_util::stream::iter(ids.into_iter().map(|work_item_id| {
        let client = client.clone();
        let config = config.clone();
        async move {
            let result =
                handle_queued_directed_evolution_work_item(&client, &config, &work_item_id).await;
            (work_item_id, result)
        }
    }))
    .buffer_unordered(concurrency)
    .collect::<Vec<_>>()
    .await;

    let mut failure_count = 0usize;
    for (work_item_id, result) in results {
        if let Err(error) = result {
            failure_count += 1;
            warn!(
                work_item_id = %work_item_id,
                %error,
                "Directed Evolution WorkItem pickup failed"
            );
        }
    }
    if failure_count > 0 {
        bail!(
            "{} Directed Evolution WorkItem pickup task(s) failed",
            failure_count
        );
    }

    Ok(())
}

async fn query_boot_entity_ids(
    client: &reqwest::Client,
    config: &Config,
    entity_set: &str,
    status: &str,
) -> Result<Vec<String>> {
    let url = format!(
        "{}/tdata/{}?$filter=Status eq '{}'&$orderby=Id desc&$top=50",
        config.temper_url, entity_set, status
    );
    let response = client
        .get(url)
        .headers(headers(config)?)
        .send()
        .await
        .with_context(|| format!("query {entity_set} with status {status} on boot"))?;
    if !response.status().is_success() {
        warn!(entity_set, status, http_status = %response.status(), "boot query failed");
        return Ok(Vec::new());
    }

    let body: Value = response
        .json()
        .await
        .with_context(|| format!("parse {entity_set} boot query"))?;
    let ids = body
        .get("value")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let fields = item.get("fields").cloned().unwrap_or_else(|| json!({}));
                    let id = first_string(item, &fields, &["entity_id", "id", "Id"], &["id", "Id"]);
                    if id.is_empty() { None } else { Some(id) }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ids)
}

async fn watch_events(client: &reqwest::Client, config: &Config) -> Result<()> {
    let mut last_error = None;
    for url in config.events_urls() {
        match connect_and_watch_events(client, config, &url).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!(%error, url, "Temper event stream candidate failed");
                last_error = Some(error);
            }
        }
    }

    if let Some(error) = last_error {
        Err(error)
    } else {
        bail!("no Temper event stream URLs were configured")
    }
}
