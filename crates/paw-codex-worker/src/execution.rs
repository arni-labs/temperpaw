async fn run_repo_sweep(
    client: &reqwest::Client,
    config: &Config,
    worker_run: &WorkerRunState,
    snapshot_id: &str,
) -> Result<String> {
    let workdir = ensure_worktree(config, worker_run).await?;

    info!(
        worker_run_id = %worker_run.id,
        snapshot_id,
        root = %workdir.display(),
        "starting local Codex repo-health patrol"
    );

    let prompt = repo_health_agent_prompt(snapshot_id, worker_run);
    let output = run_codex_exec_command(
        config,
        &workdir,
        prompt,
        "run local Codex repo-health patrol",
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n\n[stderr]\n{stderr}")
    };
    if !output.status.success() {
        bail!(
            "repo-health Codex patrol failed with status {:?}: {}",
            output.status.code(),
            truncate_middle(&combined, 4_000)
        );
    }

    let agent_output = parse_repo_health_agent_output(&combined)?;
    let graph_json = serde_json::to_string(&agent_output.graph)
        .context("serialize agent-led repo sweep graph")?;
    let finding_count =
        agent_output.graph.quality_findings.len() + agent_output.graph.security_findings.len();
    let summary = repo_sweep_summary_markdown(&agent_output);

    post_entity_action(
        client,
        config,
        "RepoGraphSnapshots",
        snapshot_id,
        "ScanComplete",
        json!({
            "graph_json": graph_json,
            "summary_markdown": summary,
            "generated_at": generated_at_label(),
            "finding_count": finding_count.to_string()
        }),
    )
    .await?;

    Ok(format!(
        "Agent-led repo-health patrol completed for RepoGraphSnapshot {snapshot_id}: {} quality finding(s), {} security finding(s).\n\n{}",
        agent_output.graph.quality_findings.len(),
        agent_output.graph.security_findings.len(),
        repo_sweep_summary_markdown(&agent_output)
    ))
}

async fn repo_sweep_live_e2e_summary(
    client: &reqwest::Client,
    config: &Config,
    worker_run: &WorkerRunState,
) -> Result<String> {
    let snapshot_id = extract_repo_sweep_snapshot_id(&worker_run.task)
        .context("WorkerRun task did not include RepoGraphSnapshot id")?;
    let response = client
        .get(config.entity_url("RepoGraphSnapshots", &snapshot_id))
        .headers(headers(config)?)
        .send()
        .await
        .context("fetch RepoGraphSnapshot for live evidence")?;
    if !response.status().is_success() {
        bail!("fetch RepoGraphSnapshot returned {}", response.status());
    }
    let value: Value = response.json().await.context("parse RepoGraphSnapshot")?;
    let fields = value.get("fields").cloned().unwrap_or_else(|| json!({}));
    let status = first_string(
        &value,
        &fields,
        &["status", "Status"],
        &["status", "Status"],
    );
    let finding_count = first_string(
        &value,
        &fields,
        &["finding_count", "FindingCount"],
        &["finding_count", "FindingCount"],
    );
    Ok(format!(
        "RepoGraphSnapshot {snapshot_id} is {status} with finding_count={}. WorkerRun {} self-reported through Temper actions.",
        if finding_count.is_empty() {
            "unknown"
        } else {
            finding_count.as_str()
        },
        worker_run.id
    ))
}

#[cfg(test)]
async fn run_codex(config: &Config, worker_run: &WorkerRunState) -> Result<String> {
    if !config.enable_execution {
        return Ok(format!(
            "dry-run: would run codex for WorkerRun {}. Set PAW_CODEX_ENABLE_EXECUTION=1 to execute.",
            worker_run.id
        ));
    }

    let workdir = ensure_worktree(config, worker_run).await?;
    let task = if worker_run.task.is_empty() {
        "Inspect this WorkerRun and report that no task was provided.".to_string()
    } else {
        worker_run.task.clone()
    };

    run_codex_implementation(config, worker_run, &workdir, task).await
}

async fn run_codex_for_worker(
    client: &reqwest::Client,
    config: &Config,
    worker_run: &WorkerRunState,
) -> Result<String> {
    if !config.enable_execution {
        return Ok(format!(
            "dry-run: would run codex for WorkerRun {}. Set PAW_CODEX_ENABLE_EXECUTION=1 to execute.",
            worker_run.id
        ));
    }

    let workdir = ensure_worktree(config, worker_run).await?;
    let task = if worker_run.task.is_empty() {
        "Inspect this WorkerRun and report that no task was provided.".to_string()
    } else {
        worker_run.task.clone()
    };
    let plan = run_codex_plan_mode(config, worker_run, &workdir, &task).await?;
    revise_work_cycle_plan(client, config, worker_run, &plan).await?;
    run_codex_implementation(
        config,
        worker_run,
        &workdir,
        implementation_prompt_with_plan(&task, &plan),
    )
    .await
}

async fn run_codex_implementation(
    config: &Config,
    worker_run: &WorkerRunState,
    workdir: &Path,
    task: String,
) -> Result<String> {
    let pull_request_mode = pull_request_mode_from_env();

    info!(worker_run_id = %worker_run.id, workdir = %workdir.display(), "starting local codex");
    let output = run_codex_exec_command(config, workdir, task, "run local codex exec").await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "codex exec failed with status {:?}: {}{}{}",
            output.status.code(),
            stdout,
            if stdout.is_empty() || stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            stderr
        );
    }

    let evidence = match collect_worktree_evidence(workdir).await {
        Ok(evidence) => evidence,
        Err(error) if pull_request_mode == PullRequestMode::Required => {
            return Err(error).context("collect git evidence before required PR publication");
        }
        Err(error) => WorktreeEvidence {
            status_short: format!("git evidence unavailable: {error}"),
            diff_stat: String::new(),
        },
    };
    ensure_no_forbidden_done_paths(&evidence.status_short)?;
    let pull_request =
        publish_pull_request_if_needed(config, worker_run, workdir, &evidence, pull_request_mode)
            .await?;
    Ok(format_codex_success_summary(
        worker_run,
        workdir,
        stdout.as_ref(),
        &evidence,
        pull_request.as_ref(),
    ))
}

async fn collect_worktree_evidence(workdir: &Path) -> Result<WorktreeEvidence> {
    let status_short = git_capture(workdir, &["status", "--short"])
        .await
        .context("git status --short")?;
    let diff_stat = git_capture(workdir, &["diff", "--stat", "--"])
        .await
        .context("git diff --stat")?;

    Ok(WorktreeEvidence {
        status_short,
        diff_stat,
    })
}

fn format_codex_success_summary(
    worker_run: &WorkerRunState,
    workdir: &Path,
    stdout: &str,
    evidence: &WorktreeEvidence,
    pull_request: Option<&PullRequestEvidence>,
) -> String {
    let branch = worker_run_branch_label(worker_run);
    let stdout_tail = nonempty_block(&tail_string(stdout.trim(), 4_000), "(no stdout captured)");
    let status_short = nonempty_block(&evidence.status_short, "(clean worktree)");
    let diff_stat = nonempty_block(&evidence.diff_stat, "(no unstaged diff stat)");
    let pull_request_block = pull_request
        .map(format_pull_request_evidence)
        .unwrap_or_default();

    format!(
        "codex exec completed for WorkerRun {}\nBranch: {branch}\nWorktree: {}\n{pull_request_block}\n\nCodex stdout:\n{stdout_tail}\n\nWorktree evidence:\n```git-status\n{status_short}\n```\n\n```git-diff-stat\n{diff_stat}\n```",
        worker_run.id,
        workdir.display()
    )
}

async fn git_capture(workdir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed with status {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn nonempty_block(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.trim_end().to_string()
    }
}

fn worker_run_branch_label(worker_run: &WorkerRunState) -> String {
    if !worker_run.branch_name.trim().is_empty() {
        worker_run.branch_name.clone()
    } else if !worker_run.worktree_path.trim().is_empty() {
        "(assigned worktree without branch)".to_string()
    } else {
        "(unassigned worktree)".to_string()
    }
}

fn ensure_no_forbidden_done_paths(status_short: &str) -> Result<()> {
    let raw_prefixes = env::var("PAW_CODEX_FORBIDDEN_DONE_PATHS").unwrap_or_default();
    let violations = forbidden_done_path_violations(status_short, &raw_prefixes);
    if violations.is_empty() {
        return Ok(());
    }

    bail!(
        "WorkerRun changed deployment-sensitive path(s) forbidden by PAW_CODEX_FORBIDDEN_DONE_PATHS: {}. Refusing WorkerRun.ReportDone.",
        violations.join(", ")
    )
}

fn forbidden_done_path_violations(status_short: &str, raw_prefixes: &str) -> Vec<String> {
    let prefixes: Vec<&str> = raw_prefixes
        .split([',', '\n'])
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .collect();
    if prefixes.is_empty() {
        return Vec::new();
    }

    let mut violations = Vec::new();
    for path in changed_paths_from_status(status_short) {
        if prefixes.iter().any(|prefix| path.starts_with(prefix)) {
            violations.push(path);
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

fn changed_paths_from_status(status_short: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in status_short.lines() {
        let Some(rest) = line.get(3..) else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        if let Some((before, after)) = rest.split_once(" -> ") {
            paths.push(before.trim_matches('"').to_string());
            paths.push(after.trim_matches('"').to_string());
        } else {
            paths.push(rest.trim_matches('"').to_string());
        }
    }
    paths
}

async fn run_codex_review(
    config: &Config,
    worker_run: &WorkerRunState,
    review_run: &ReviewRunState,
) -> Result<ReviewDecision> {
    let workdir = ensure_worktree(config, worker_run).await?;
    let prompt = codex_review_prompt(worker_run, review_run);

    info!(
        worker_run_id = %worker_run.id,
        workdir = %workdir.display(),
        "starting local Codex reviewer"
    );
    let output = run_codex_exec_command(config, &workdir, prompt, "run local codex review").await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n\n[stderr]\n{stderr}")
    };
    if !output.status.success() {
        bail!(
            "codex review failed with status {:?}: {}",
            output.status.code(),
            truncate_middle(&combined, 4_000)
        );
    }

    Ok(parse_codex_review_verdict(&combined))
}

async fn run_codex_exec_command(
    config: &Config,
    workdir: &Path,
    prompt: String,
    context_label: &str,
) -> Result<Output> {
    run_codex_exec_command_with_args(
        config,
        workdir,
        codex_exec_args(workdir, &prompt),
        context_label,
    )
    .await
}

async fn run_codex_exec_command_with_args(
    config: &Config,
    workdir: &Path,
    args: Vec<std::ffi::OsString>,
    context_label: &str,
) -> Result<Output> {
    let mut command = Command::new(&config.codex_bin);
    command
        .args(&args)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if args
        .iter()
        .any(|arg| arg == "forced_login_method=\"chatgpt\"")
    {
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "AZURE_OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "WORKER_TOKEN",
            "DSF_FACTORY_AGENT_TOKEN",
        ] {
            command.env_remove(name);
        }
        command.env("TEMPER_API_KEY", required_env("DSF_FACTORY_AGENT_TOKEN")?);
    }
    configure_process_group(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("{context_label}: spawn codex exec"))?;
    let child_pid = child.id();
    let mut stdout_pipe = child
        .stdout
        .take()
        .with_context(|| format!("{context_label}: capture codex stdout"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .with_context(|| format!("{context_label}: capture codex stderr"))?;
    let stdout_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        stdout_pipe.read_to_end(&mut stdout).await?;
        std::io::Result::Ok(stdout)
    });
    let stderr_task = tokio::spawn(async move {
        let mut stderr = Vec::new();
        stderr_pipe.read_to_end(&mut stderr).await?;
        std::io::Result::Ok(stderr)
    });

    match timeout(config.codex_exec_timeout, child.wait()).await {
        Ok(status) => {
            let status = status.with_context(|| context_label.to_string())?;
            let stdout = stdout_task
                .await
                .with_context(|| format!("{context_label}: join codex stdout reader"))?
                .with_context(|| format!("{context_label}: read codex stdout"))?;
            let stderr = stderr_task
                .await
                .with_context(|| format!("{context_label}: join codex stderr reader"))?
                .with_context(|| format!("{context_label}: read codex stderr"))?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        }
        Err(_) => {
            if let Some(pid) = child_pid {
                terminate_process_group(pid, "timed-out Codex process group").await;
            }
            let _ = timeout(Duration::from_secs(5), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            bail!(
                "codex exec timed out after {}s during {context_label}",
                config.codex_exec_timeout.as_secs()
            )
        }
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
async fn terminate_process_group(pid: u32, context: &str) {
    signal_process_group(pid, libc::SIGTERM, "TERM", context);
    sleep(Duration::from_millis(250)).await;
    signal_process_group(pid, libc::SIGKILL, "KILL", context);
    sleep(Duration::from_millis(50)).await;
}

#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int, label: &str, context: &str) {
    let process_group = -(pid as libc::pid_t);
    let result = unsafe { libc::kill(process_group, signal) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            warn!(%error, pid, signal = label, context, "failed to signal process group");
        }
    }
}

#[cfg(not(unix))]
async fn terminate_process_group(_pid: u32, _context: &str) {}

fn codex_exec_args(workdir: &Path, prompt: &str) -> Vec<std::ffi::OsString> {
    let mut args = vec!["exec".into(), "--ignore-user-config".into()];
    if let Some(datadog_mcp_url) = codex_datadog_mcp_url() {
        args.extend([
            "-c".into(),
            format!(
                "mcp_servers.datadog.url={}",
                toml_basic_string(&datadog_mcp_url)
            )
            .into(),
        ]);
    }
    args.extend([
        "--ephemeral".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "--cd".into(),
        workdir.as_os_str().to_os_string(),
        "--skip-git-repo-check".into(),
        prompt.into(),
    ]);
    args
}

fn codex_datadog_mcp_url() -> Option<String> {
    let enabled = env::var("PAW_CODEX_ENABLE_DATADOG_MCP")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    Some(
        env::var("PAW_CODEX_DATADOG_MCP_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                "https://mcp.datadoghq.com/api/unstable/mcp-server/mcp?toolsets=all".to_string()
            }),
    )
}

fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn codex_review_prompt(worker_run: &WorkerRunState, review_run: &ReviewRunState) -> String {
    if let Some(snapshot_id) = extract_repo_sweep_snapshot_id(&worker_run.task) {
        return codex_repo_health_review_prompt(worker_run, review_run, &snapshot_id);
    }

    format!(
        "You are the independent reviewer for a TemperPaw paw-patrol WorkerRun.\n\nWorkerRun: {}\nReviewRun worker reference: {}\nProofPacket: {}\nBranch: {}\n\nTask:\n{}\n\nReview requirements:\n1. Treat this as read-only review. Do not modify files.\n2. Inspect the git diff, changed files, tests/proofs, and Temper-native architecture constraints.\n3. Run targeted checks or live/E2E verification when relevant and mention exactly what you ran.\n4. Look for security, Cedar, WASM, Discord/user-facing, dependency, and readability risks.\n5. Do not require production deployment to approve a PR-ready code/config change when deployment is intentionally human-gated or risk-gated; record that as a deployment-pending residual risk in SUMMARY or LIVE_E2E instead.\n6. Return an explicit verdict marker on its own line: VERDICT: approve, VERDICT: request_changes, or VERDICT: escalate.\n7. Include SUMMARY: and LIVE_E2E: lines that are concise and human-readable.\n\nIf you cannot confidently approve the proposed change itself, use request_changes or escalate.",
        worker_run.id,
        review_run.worker_run_id,
        if review_run.proof_packet_id.is_empty() {
            "(not attached yet)"
        } else {
            review_run.proof_packet_id.as_str()
        },
        worker_run_branch_label(worker_run),
        if worker_run.task.is_empty() {
            "(no task text recorded)"
        } else {
            worker_run.task.as_str()
        }
    )
}

fn codex_repo_health_review_prompt(
    worker_run: &WorkerRunState,
    review_run: &ReviewRunState,
    snapshot_id: &str,
) -> String {
    format!(
        "You are the independent repo-health Patrol scan reviewer for TemperPaw paw-patrol.\n\nRepoGraphSnapshot: {snapshot_id}\nWorkerRun: {}\nReviewRun worker reference: {}\nProofPacket: {}\nBranch: {}\n\nTask:\n{}\n\nReview requirements:\n1. Treat this as read-only review. Do not modify files, and do not require an implementation patch for a repo-health patrol scan.\n2. Verify the worker satisfied the scan contract: Codex investigated the repo with agent judgment, returned structured JSON, the worker self-reported RepoGraphSnapshot.ScanComplete, and the snapshot opened evidenced QualityFinding/SecurityFinding entities.\n3. Inspect the scan evidence, summary/proof, entity links, and any git status needed to confirm the scan did not silently edit files.\n4. Run targeted read-only checks when useful, such as `git status --short`, `rg` evidence spot checks, or OData entity reads; mention exactly what you ran.\n5. Look for hallucinated findings, missing evidence surfaces, secret leakage, missing RepoGraphSnapshot linkage, or unsafe auto-start of high-risk cleanup work.\n6. Return an explicit verdict marker on its own line: VERDICT: approve, VERDICT: request_changes, or VERDICT: escalate.\n7. Include SUMMARY: and LIVE_E2E: lines that are concise and human-readable.\n\nIf the scan evidence is incomplete, inconsistent, or unsafe, request changes or escalate.",
        worker_run.id,
        review_run.worker_run_id,
        if review_run.proof_packet_id.is_empty() {
            "(not attached yet)"
        } else {
            review_run.proof_packet_id.as_str()
        },
        worker_run_branch_label(worker_run),
        if worker_run.task.is_empty() {
            "(no task text recorded)"
        } else {
            worker_run.task.as_str()
        }
    )
}

fn parse_codex_review_verdict(output: &str) -> ReviewDecision {
    let raw_verdict = prefixed_line(output, "VERDICT:")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('-', "_");
    let summary = prefixed_line(output, "SUMMARY:")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| truncate_middle(output.trim(), 2_000));
    let live_e2e_summary = prefixed_line(output, "LIVE_E2E:")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Reviewer did not provide a LIVE_E2E line.".to_string());

    match raw_verdict.trim() {
        "approve" | "approved" => ReviewDecision {
            action: ReviewDecisionAction::Approve,
            summary,
            live_e2e_summary,
            verdict: "approve".to_string(),
        },
        "request_changes" | "changes_requested" | "request changes" => ReviewDecision {
            action: ReviewDecisionAction::RequestChanges,
            summary,
            live_e2e_summary,
            verdict: "request_changes".to_string(),
        },
        "escalate" | "human" | "human_review" => ReviewDecision {
            action: ReviewDecisionAction::Escalate,
            summary,
            live_e2e_summary,
            verdict: "escalate".to_string(),
        },
        _ => ReviewDecision {
            action: ReviewDecisionAction::Escalate,
            summary: format!(
                "Codex reviewer did not provide an explicit VERDICT marker. Raw output: {}",
                truncate_middle(output.trim(), 2_000)
            ),
            live_e2e_summary,
            verdict: "escalate_missing_verdict".to_string(),
        },
    }
}

fn prefixed_line(output: &str, prefix: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix(prefix)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn tail_string(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        value.to_string()
    } else {
        chars[chars.len() - max_chars..].iter().collect()
    }
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }
    let head = max_chars / 2;
    let tail = max_chars.saturating_sub(head + 15);
    format!(
        "{}\n...[truncated]...\n{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

async fn ensure_worktree(config: &Config, worker_run: &WorkerRunState) -> Result<PathBuf> {
    if worker_run.worktree_path.is_empty() && worker_run.branch_name.trim().is_empty() {
        bail!(
            "WorkerRun {} has no assigned worktree_path or branch_name; refusing to run in the main checkout",
            worker_run.id
        );
    }

    let worktree = if worker_run.worktree_path.is_empty() {
        config
            .workspace_root
            .join(worker_run.branch_name.replace('/', "-"))
    } else {
        PathBuf::from(&worker_run.worktree_path)
    };
    if worktree.exists() {
        return Ok(worktree);
    }

    if worker_run.branch_name.trim().is_empty() {
        bail!(
            "WorkerRun {} has worktree_path but no branch_name",
            worker_run.id
        );
    }

    if let Some(parent) = worktree.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    info!(
        worker_run_id = %worker_run.id,
        branch = %worker_run.branch_name,
        worktree = %worktree.display(),
        repo = %config.repo_root.display(),
        "creating git worktree"
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(&config.repo_root)
        .arg("worktree")
        .arg("add")
        .arg("-B")
        .arg(&worker_run.branch_name)
        .arg(&worktree)
        .arg("HEAD")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("git worktree add")?;

    if !output.status.success() {
        bail!(
            "git worktree add failed with status {:?}: {}{}{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            if output.stdout.is_empty() || output.stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(worktree)
}

fn generated_at_label() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("unix:{seconds}")
}
