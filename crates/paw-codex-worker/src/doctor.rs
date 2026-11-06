async fn run_doctor(client: &reqwest::Client, config: &Config) -> Result<()> {
    let mut checks = Vec::new();
    checks.push(check_path("repo_root", &config.repo_root, true));
    checks.push(check_path("workspace_root", &config.workspace_root, false));
    checks.push(check_worker_token(config));
    checks.push(check_worker_capabilities());
    checks.push(check_datadog_mcp_contract());
    checks.push(check_execution_safety(config));
    checks.push(check_codex_binary(config).await);
    checks.push(check_codex_exec_smoke(config).await);
    checks.push(check_odata(client, config).await);
    checks.push(check_event_stream(client, config).await);

    print_doctor_report(config, &checks);
    if doctor_has_failures(&checks) {
        bail!("paw-codex-worker doctor found failing checks")
    }
    Ok(())
}

fn check_worker_capabilities() -> DoctorCheck {
    let capabilities = worker_capabilities();
    if capabilities.is_empty() {
        DoctorCheck::fail(
            "worker_capabilities",
            "PAW_CODEX_WORKER_CAPABILITIES resolved to an empty set".to_string(),
        )
    } else if capabilities.iter().any(|capability| capability == "datadog_query")
        && capabilities.iter().any(|capability| capability == "github_query")
    {
        DoctorCheck::pass(
            "worker_capabilities",
            format!("advertising {}", capabilities.join(",")),
        )
    } else {
        DoctorCheck::warn(
            "worker_capabilities",
            format!(
                "advertising {}; Datadog Patrol requires datadog_query and GitHub Patrol requires github_query",
                capabilities.join(",")
            ),
        )
    }
}

fn check_datadog_mcp_contract() -> DoctorCheck {
    let capabilities = worker_capabilities();
    let has_datadog = capabilities
        .iter()
        .any(|capability| capability == "datadog_query");
    let has_github = capabilities
        .iter()
        .any(|capability| capability == "github_query");
    if has_datadog && has_github {
        DoctorCheck::pass(
            "patrol_query_capabilities",
            "worker advertises datadog_query and github_query; Patrol will use local Codex agent contracts"
                .to_string(),
        )
    } else {
        let missing = [
            (!has_datadog).then_some("datadog_query"),
            (!has_github).then_some("github_query"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",");
        DoctorCheck::warn(
            "patrol_query_capabilities",
            format!(
                "worker is missing {missing}; Patrol runs with that capability will not be claimed"
            ),
        )
    }
}

fn check_path(name: &str, path: &Path, must_be_git_repo: bool) -> DoctorCheck {
    if !path.exists() {
        return DoctorCheck::fail(name, format!("{} does not exist", path.display()));
    }
    if !path.is_dir() {
        return DoctorCheck::fail(name, format!("{} is not a directory", path.display()));
    }
    if must_be_git_repo && !path.join(".git").exists() {
        return DoctorCheck::warn(
            name,
            format!(
                "{} exists but does not have a .git entry; git worktree creation may fail",
                path.display()
            ),
        );
    }
    DoctorCheck::pass(name, format!("{} exists", path.display()))
}

fn check_worker_token(config: &Config) -> DoctorCheck {
    if config.worker_token.is_some() {
        DoctorCheck::pass("worker_token", "WORKER_TOKEN is set".to_string())
    } else {
        DoctorCheck::warn(
            "worker_token",
            "WORKER_TOKEN is not set; OData/event calls will only work against permissive local dev setups".to_string(),
        )
    }
}

fn check_execution_safety(config: &Config) -> DoctorCheck {
    if config.enable_execution {
        DoctorCheck::warn(
            "execution",
            "PAW_CODEX_ENABLE_EXECUTION=1; this worker may run Codex and local evaluation commands"
                .to_string(),
        )
    } else {
        DoctorCheck::pass(
            "execution",
            "PAW_CODEX_ENABLE_EXECUTION is off; non-sweep work will dry-run".to_string(),
        )
    }
}

async fn check_codex_binary(config: &Config) -> DoctorCheck {
    match Command::new(&config.codex_bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
    {
        Ok(output) if output.status.success() => DoctorCheck::pass(
            "codex_bin",
            format!(
                "{} is available: {}",
                config.codex_bin,
                first_nonempty_line(&String::from_utf8_lossy(&output.stdout))
                    .or_else(|| first_nonempty_line(&String::from_utf8_lossy(&output.stderr)))
                    .unwrap_or_else(|| "version command succeeded".to_string())
            ),
        ),
        Ok(output) => DoctorCheck::fail(
            "codex_bin",
            format!(
                "{} --version exited {:?}: {}{}",
                config.codex_bin,
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        ),
        Err(error) => DoctorCheck::fail(
            "codex_bin",
            format!("failed to run {} --version: {error}", config.codex_bin),
        ),
    }
}

async fn check_codex_exec_smoke(config: &Config) -> DoctorCheck {
    if !config.codex_exec_smoke {
        return DoctorCheck::pass(
            "codex_exec_smoke",
            "PAW_CODEX_DOCTOR_EXEC_SMOKE is off; exec auth/session smoke was skipped"
                .to_string(),
        );
    }

    let workdir = std::env::temp_dir().join(format!(
        "paw-codex-worker-doctor-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    if let Err(error) = fs::create_dir_all(&workdir) {
        return DoctorCheck::fail(
            "codex_exec_smoke",
            format!("failed to create smoke workdir {}: {error}", workdir.display()),
        );
    }

    let output_result = timeout(
        Duration::from_secs(45),
        Command::new(&config.codex_bin)
            .arg("exec")
            .arg("--skip-git-repo-check")
            .arg(
                "PAW_CODEX_DOCTOR_EXEC_SMOKE: reply with PAW_CODEX_DOCTOR_EXEC_OK only. Do not read or edit files.",
            )
            .current_dir(&workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await;
    fs::remove_dir_all(&workdir).ok();

    match output_result {
        Ok(Ok(output)) if output.status.success() => DoctorCheck::pass(
            "codex_exec_smoke",
            format!(
                "codex exec smoke passed: {}",
                first_nonempty_line(&String::from_utf8_lossy(&output.stdout))
                    .or_else(|| first_nonempty_line(&String::from_utf8_lossy(&output.stderr)))
                    .unwrap_or_else(|| "exec command succeeded".to_string())
            ),
        ),
        Ok(Ok(output)) => DoctorCheck::fail(
            "codex_exec_smoke",
            format!(
                "codex exec smoke exited {:?}: {}{}",
                output.status.code(),
                truncate_middle(&String::from_utf8_lossy(&output.stdout), 1_000),
                truncate_middle(&String::from_utf8_lossy(&output.stderr), 1_000)
            ),
        ),
        Ok(Err(error)) => DoctorCheck::fail(
            "codex_exec_smoke",
            format!("failed to run codex exec smoke: {error}"),
        ),
        Err(_) => DoctorCheck::fail(
            "codex_exec_smoke",
            "codex exec smoke timed out after 45s".to_string(),
        ),
    }
}

async fn check_odata(client: &reqwest::Client, config: &Config) -> DoctorCheck {
    let url = format!("{}/tdata/$metadata", config.temper_url);
    if config.worker_token.is_none() {
        return DoctorCheck::fail(
            "odata",
            "no worker token configured; /tdata/$metadata needs a credential".to_string(),
        );
    }
    match client
        .get(&url)
        .headers(event_stream_headers(config).unwrap_or_default())
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            DoctorCheck::pass("odata", format!("GET {url} returned {}", response.status()))
        }
        Ok(response) => {
            DoctorCheck::fail("odata", format!("GET {url} returned {}", response.status()))
        }
        Err(error) => DoctorCheck::fail("odata", format!("GET {url} failed: {error}")),
    }
}

async fn check_event_stream(client: &reqwest::Client, config: &Config) -> DoctorCheck {
    let Some(url) = config.events_urls().first().cloned() else {
        return DoctorCheck::fail("event_stream", "no event stream URL configured".to_string());
    };
    let request = client
        .get(&url)
        .headers(event_stream_headers(config).unwrap_or_default())
        .header(ACCEPT, "text/event-stream")
        .send();

    match tokio::time::timeout(Duration::from_secs(5), request).await {
        Ok(Ok(response)) if response.status().is_success() => DoctorCheck::pass(
            "event_stream",
            format!("GET {url} returned {}", response.status()),
        ),
        Ok(Ok(response)) => DoctorCheck::fail(
            "event_stream",
            format!("GET {url} returned {}", response.status()),
        ),
        Ok(Err(error)) => DoctorCheck::fail("event_stream", format!("GET {url} failed: {error}")),
        Err(_) => DoctorCheck::fail("event_stream", format!("GET {url} timed out after 5s")),
    }
}
