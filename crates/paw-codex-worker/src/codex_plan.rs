fn codex_plan_args(workdir: &Path, prompt: &str) -> Vec<std::ffi::OsString> {
    vec![
        "exec".into(),
        "--ignore-user-config".into(),
        "--ephemeral".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--cd".into(),
        workdir.as_os_str().to_os_string(),
        "--skip-git-repo-check".into(),
        prompt.into(),
    ]
}

fn codex_plan_prompt(worker_run: &WorkerRunState, task: &str) -> String {
    format!(
        "You are in Codex Plan Mode for a Patrol WorkCycle. Do not modify files, do not install packages, do not run destructive commands, and do not dispatch Temper actions.\n\nWorkerRun: {}\nWorkCycle: {}\nBranch: {}\n\nOriginal implementation task:\n{}\n\nProduce a focused markdown plan that another Codex implementer can execute without guessing. Use exactly these sections:\n\n## Context\nExplain the issue, user-facing impact, and relevant entity links.\n\n## Exploration Summary\nList the files, specs, policies, routes, tests, and runtime surfaces that must be read before editing.\n\n## Approach\nDescribe the smallest Temper-native implementation path and how it respects entity state machines, WASM integrations, and Cedar policies.\n\n## File Manifest\nList every expected file area to inspect or change, with why.\n\n## Verification Plan\nName the red test, green implementation checks, focused commands, dashboard/API checks, and live/E2E proof needed.\n\n## Risks\nCall out approval, deployment, data, policy, Discord/transport, and dashboard risks plus mitigations.\n\n## Open Questions\nList unknowns that must be answered before or during implementation.",
        worker_run.id,
        non_empty_or(&worker_run.work_cycle_id, "unknown"),
        worker_run_branch_label(worker_run),
        task
    )
}

fn implementation_prompt_with_plan(task: &str, plan: &str) -> String {
    format!(
        "{task}\n\n<active_workcycle_plan>\n{plan}\n</active_workcycle_plan>\n\nExecute the active WorkCycle plan. If exploration proves the plan wrong, update course deliberately and explain the change in the WorkerRun proof."
    )
}

async fn run_codex_plan_mode(
    config: &Config,
    worker_run: &WorkerRunState,
    workdir: &Path,
    task: &str,
) -> Result<String> {
    let prompt = codex_plan_prompt(worker_run, task);
    info!(
        worker_run_id = %worker_run.id,
        work_cycle_id = %worker_run.work_cycle_id,
        workdir = %workdir.display(),
        "starting read-only Codex Plan Mode"
    );
    let output = run_codex_exec_command_with_args(
        config,
        workdir,
        codex_plan_args(workdir, &prompt),
        "run local codex plan mode",
        &[],
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "codex plan mode failed with status {:?}: {}{}{}",
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
    let plan = stdout.trim();
    if plan.is_empty() {
        bail!("codex plan mode produced an empty WorkCycle plan");
    }
    Ok(plan.to_string())
}

async fn revise_work_cycle_plan(
    client: &reqwest::Client,
    config: &Config,
    worker_run: &WorkerRunState,
    plan: &str,
) -> Result<()> {
    if worker_run.work_cycle_id.trim().is_empty() {
        bail!(
            "WorkerRun {} cannot attach Codex Plan Mode output because work_cycle_id is empty",
            worker_run.id
        );
    }
    post_entity_action(
        client,
        config,
        "WorkCycles",
        &worker_run.work_cycle_id,
        "RevisePlan",
        json!({ "plan_summary": plan }),
    )
    .await
    .with_context(|| {
        format!(
            "attach Codex Plan Mode output to WorkCycle {}",
            worker_run.work_cycle_id
        )
    })
}
