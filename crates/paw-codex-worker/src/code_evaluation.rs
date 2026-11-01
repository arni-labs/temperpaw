async fn run_code_change_evaluation(
    client: &reqwest::Client,
    config: &Config,
    evaluation_run_id: &str,
    worker_run: &WorkerRunState,
) -> Result<()> {
    info!(
        evaluation_run_id,
        action = EVALUATION_START_LABEL,
        "starting code-change EvaluationRun"
    );
    post_entity_action(
        client,
        config,
        "EvaluationRuns",
        evaluation_run_id,
        "Start",
        json!({}),
    )
    .await?;

    let outcome = run_evaluation_commands(config, worker_run).await?;
    if outcome.passed {
        post_entity_action(
            client,
            config,
            "EvaluationRuns",
            evaluation_run_id,
            "Pass",
            json!({
                "results_json": outcome.results_json,
                "e2e_summary": outcome.e2e_summary,
            }),
        )
        .await
    } else {
        post_entity_action(
            client,
            config,
            "EvaluationRuns",
            evaluation_run_id,
            "Fail",
            json!({
                "results_json": outcome.results_json,
                "error_message": outcome.error_message,
                "failure_classification": outcome.failure_classification,
            }),
        )
        .await
    }
}

async fn run_evaluation_commands(
    config: &Config,
    worker_run: &WorkerRunState,
) -> Result<EvaluationOutcome> {
    let commands = evaluation_commands(env::var("PAW_CODEX_EVAL_COMMANDS").ok().as_deref());
    run_evaluation_command_list(config, worker_run, commands).await
}

async fn run_evaluation_command_list(
    config: &Config,
    worker_run: &WorkerRunState,
    commands: Vec<String>,
) -> Result<EvaluationOutcome> {
    let workdir = ensure_worktree(config, worker_run).await?;
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut results = Vec::new();
    let mut passed = true;
    let mut failure_classification = "passed";

    for command in commands {
        info!(worker_run_id = %worker_run.id, command, "running evaluation command");
        let output = run_shell_command_with_timeout(config, &workdir, &shell, &command).await?;
        let status = output.status_code;
        let success = output.success;
        passed &= success;
        if output.timed_out {
            failure_classification = "evaluator_timeout";
        } else if !success && failure_classification == "passed" {
            failure_classification = "command_exit_failure";
        }
        results.push(json!({
            "command": command,
            "success": success,
            "status": status,
            "timed_out": output.timed_out,
            "timeout_ms": config.codex_exec_timeout.as_millis(),
            "failure_classification": if output.timed_out {
                "evaluator_timeout"
            } else if success {
                "passed"
            } else {
                "command_exit_failure"
            },
            "stdout_tail": tail_string(&String::from_utf8_lossy(&output.stdout), 4_000),
            "stderr_tail": tail_string(&String::from_utf8_lossy(&output.stderr), 4_000),
        }));
    }

    let results_json = serde_json::to_string(&json!({
        "kind": "code_change_evaluation",
        "worker_run_id": worker_run.id,
        "failure_classification": failure_classification,
        "timeout_ms": config.codex_exec_timeout.as_millis(),
        "commands": results,
    }))
    .context("serialize code-change evaluation result")?;
    let e2e_summary = if passed {
        format!(
            "Code-change evaluation passed for WorkerRun {} with configured local commands.",
            worker_run.id
        )
    } else {
        format!(
            "Code-change evaluation failed for WorkerRun {} with failure_classification={}; inspect results_json command output.",
            worker_run.id, failure_classification
        )
    };
    let error_message = if passed {
        String::new()
    } else {
        e2e_summary.clone()
    };

    Ok(EvaluationOutcome {
        passed,
        results_json,
        e2e_summary,
        error_message,
        failure_classification: failure_classification.to_string(),
    })
}

#[derive(Debug)]
struct ShellCommandOutput {
    status_code: i32,
    success: bool,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run_shell_command_with_timeout(
    config: &Config,
    workdir: &Path,
    shell: &str,
    command: &str,
) -> Result<ShellCommandOutput> {
    let mut shell_command = Command::new(shell);
    shell_command
        .arg("-lc")
        .arg(command)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut shell_command);

    let mut child = shell_command
        .spawn()
        .with_context(|| format!("spawn evaluation command: {command}"))?;
    let child_pid = child.id();
    let mut stdout_pipe = child
        .stdout
        .take()
        .with_context(|| format!("capture evaluation command stdout: {command}"))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .with_context(|| format!("capture evaluation command stderr: {command}"))?;
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
            let status = status.with_context(|| format!("run evaluation command: {command}"))?;
            let stdout = stdout_task
                .await
                .with_context(|| format!("join evaluation command stdout reader: {command}"))?
                .with_context(|| format!("read evaluation command stdout: {command}"))?;
            let stderr = stderr_task
                .await
                .with_context(|| format!("join evaluation command stderr reader: {command}"))?
                .with_context(|| format!("read evaluation command stderr: {command}"))?;
            Ok(ShellCommandOutput {
                status_code: status.code().unwrap_or(-1),
                success: status.success(),
                timed_out: false,
                stdout,
                stderr,
            })
        }
        Err(_) => {
            if let Some(pid) = child_pid {
                terminate_process_group(pid, "timed-out evaluation command process group").await;
            }
            let _ = timeout(Duration::from_secs(5), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            Ok(ShellCommandOutput {
                status_code: -1,
                success: false,
                timed_out: true,
                stdout: Vec::new(),
                stderr: format!(
                    "evaluation command timed out after {}",
                    duration_label(config.codex_exec_timeout)
                )
                .into_bytes(),
            })
        }
    }
}

fn duration_label(duration: Duration) -> String {
    if duration.as_millis() < 1_000 {
        format!("{}ms", duration.as_millis())
    } else if duration.subsec_millis() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn evaluation_commands(configured: Option<&str>) -> Vec<String> {
    configured
        .unwrap_or("cargo test -p temperpaw --test paw_patrol_foundation -- --nocapture")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}
