const ACTION_CLAIM_LABEL: &str = "WorkerRun.Claim";
const ACTION_REPORT_DONE_LABEL: &str = "WorkerRun.ReportDone";
const ACTION_REPORT_FAILED_LABEL: &str = "WorkerRun.ReportFailed";
const REVIEW_CLAIM_LABEL: &str = "ReviewRun.Claim";
const EVALUATION_CLAIM_LABEL: &str = "EvaluationRun.Claim";
const EVALUATION_START_LABEL: &str = "EvaluationRun.Start";
const EVALUATION_PASS_LABEL: &str = "EvaluationRun.Pass";
const EVALUATION_FAIL_LABEL: &str = "EvaluationRun.Fail";
const DIRECTED_EVOLUTION_NAMESPACE: &str = "Temper.DirectedEvolution";
const PAW_ORCHESTRATION_NAMESPACE: &str = "Temper.PawOrchestration";
const DIRECTED_EVOLUTION_WORKER_PROVIDER_ID: &str = "local_codex";

#[derive(Clone, Debug)]
struct Config {
    temper_url: String,
    tenant: String,
    worker_id: String,
    worker_token: Option<String>,
    workspace_root: PathBuf,
    repo_root: PathBuf,
    codex_bin: String,
    max_concurrent_runs: usize,
    enable_execution: bool,
    poll_on_start: bool,
    codex_exec_smoke: bool,
    codex_exec_timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkerCommand {
    Run,
    Doctor,
    LaunchdPlist,
    DirectedEvolutionStartEpisode { contract_path: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DoctorCheck {
    name: String,
    status: DoctorStatus,
    detail: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let temper_url = required_env("TEMPER_URL")?
            .trim_end_matches('/')
            .to_string();
        let tenant = env::var("TEMPER_TENANT").unwrap_or_else(|_| "default".to_string());
        let worker_id = required_env("WORKER_ID")?;
        let worker_token = env::var("WORKER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        let workspace_root = PathBuf::from(env::var("WORKSPACE_ROOT").unwrap_or_else(|_| {
            "/Users/openclaw/Development/temperpaw-worktrees".to_string()
        }));
        let repo_root = PathBuf::from(
            env::var("REPO_ROOT")
                .unwrap_or_else(|_| "/Users/openclaw/Development/temperpaw".to_string()),
        );
        let codex_bin = env::var("CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
        let max_concurrent_runs = env::var("MAX_CONCURRENT_RUNS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);
        let enable_execution = env::var("PAW_CODEX_ENABLE_EXECUTION")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let poll_on_start = env::var("PAW_CODEX_POLL_ON_START")
            .map(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        let codex_exec_smoke = env::var("PAW_CODEX_DOCTOR_EXEC_SMOKE")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let codex_exec_timeout = env::var("PAW_CODEX_EXEC_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(20 * 60));

        if max_concurrent_runs != 1 {
            bail!("MAX_CONCURRENT_RUNS must be 1 in v1 so WorkerRun review remains simple");
        }

        Ok(Self {
            temper_url,
            tenant,
            worker_id,
            worker_token,
            workspace_root,
            repo_root,
            codex_bin,
            max_concurrent_runs,
            enable_execution,
            poll_on_start,
            codex_exec_smoke,
            codex_exec_timeout,
        })
    }

    fn events_urls(&self) -> Vec<String> {
        if let Ok(path) = env::var("TEMPER_EVENTS_PATH") {
            let path = path.trim();
            if !path.is_empty() {
                return vec![self.event_url(path)];
            }
        }

        vec![
            self.event_url("/tdata/$events"),
            self.event_url("/observe/events/stream"),
        ]
    }

    fn event_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_string();
        }
        format!("{}/{}", self.temper_url, path.trim_start_matches('/'))
    }

    fn entity_url(&self, entity_set: &str, id: &str) -> String {
        format!("{}/tdata/{}('{}')", self.temper_url, entity_set, id)
    }

    fn entity_action_url_with_namespace(
        &self,
        entity_set: &str,
        id: &str,
        namespace: &str,
        action: &str,
    ) -> String {
        format!(
            "{}/tdata/{}('{}')/{}.{}",
            self.temper_url, entity_set, id, namespace, action
        )
    }
}

fn load_worker_env_file() -> Result<()> {
    let path = env::var("PAW_CODEX_WORKER_ENV_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/Users/openclaw/.config/temperpaw/paw-codex-worker.env"));
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("read PAW_CODEX_WORKER_ENV_FILE {}", path.display()))?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || (key != "PATH" && env::var_os(key).is_some()) {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        // Rust 2024 marks process environment mutation unsafe because it can
        // race with other threads. This runs before Tokio starts worker tasks.
        unsafe {
            env::set_var(key, value);
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EntityEvent {
    #[serde(default, alias = "entityType", alias = "EntityType")]
    entity_type: String,
    #[serde(default, alias = "entityId", alias = "EntityId")]
    entity_id: String,
    #[serde(
        default,
        alias = "Status",
        alias = "new_status",
        alias = "newStatus",
        alias = "to_status",
        alias = "toStatus"
    )]
    status: String,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerRunState {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "Task")]
    task: String,
    #[serde(default, rename = "WorkCycleId")]
    work_cycle_id: String,
    #[serde(default, rename = "WorktreePath")]
    worktree_path: String,
    #[serde(default, rename = "BranchName")]
    branch_name: String,
    #[serde(default, rename = "RunnerKind")]
    runner_kind: String,
    #[serde(default, rename = "AllowedWorkerId")]
    allowed_worker_id: String,
    #[serde(default, rename = "WorkerId")]
    worker_id: String,
    #[serde(default, rename = "ProviderId")]
    provider_id: String,
    #[serde(default, rename = "RequiredCapabilities")]
    required_capabilities: String,
}

#[derive(Debug, Deserialize)]
struct ReviewRunState {
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "WorkerRunId")]
    worker_run_id: String,
    #[serde(default, rename = "ProofPacketId")]
    proof_packet_id: String,
}

#[derive(Debug, Deserialize)]
struct EvaluationRunState {
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "WorkCycleId")]
    work_cycle_id: String,
    #[serde(default, rename = "EvaluatorId")]
    evaluator_id: String,
    #[serde(default, rename = "RequiredChecks")]
    required_checks: String,
}

#[derive(Debug, Deserialize)]
struct WorkCycleState {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "ImplementerWorkerRunId")]
    implementer_worker_run_id: String,
    #[serde(default, rename = "ReviewerRunId")]
    reviewer_run_id: String,
    #[serde(default, rename = "ReviewPassed")]
    review_passed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct DirectedEvolutionWorkItemState {
    #[serde(default, rename = "Id")]
    id: String,
    #[serde(default, rename = "Status")]
    status: String,
    #[serde(default, rename = "Role")]
    role: String,
    #[serde(default, rename = "TargetEntityType")]
    target_entity_type: String,
    #[serde(default, rename = "TargetEntityId")]
    target_entity_id: String,
    #[serde(default, rename = "PromptRef")]
    prompt_ref: String,
    #[serde(default, rename = "ContextRef")]
    context_ref: String,
    #[serde(default, rename = "OutputSchemaRef")]
    output_schema_ref: String,
    #[serde(default, rename = "CorrelationJson")]
    correlation_json: String,
    #[serde(default, rename = "WorkerRunId")]
    worker_run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReviewDecisionAction {
    Approve,
    RequestChanges,
    Escalate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewDecision {
    action: ReviewDecisionAction,
    summary: String,
    live_e2e_summary: String,
    verdict: String,
}

#[derive(Clone, Debug)]
struct EvaluationOutcome {
    passed: bool,
    results_json: String,
    e2e_summary: String,
    error_message: String,
    failure_classification: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EvaluationTerminalBlocker {
    results_json: String,
    error_message: String,
    failure_classification: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreeEvidence {
    status_short: String,
    diff_stat: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PullRequestMode {
    Disabled,
    Optional,
    Required,
}

impl PullRequestMode {
    fn as_str(self) -> &'static str {
        match self {
            PullRequestMode::Disabled => "disabled",
            PullRequestMode::Optional => "optional",
            PullRequestMode::Required => "required",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PullRequestEvidence {
    mode: PullRequestMode,
    url: String,
    commit_sha: String,
    branch_name: String,
    base_branch: String,
    note: String,
}
