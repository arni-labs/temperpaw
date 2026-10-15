#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::Mutex;

    include!("dsf_model_patrol_tests.rs");
    include!("datadog_patrol_tests.rs");
    include!("github_patrol_tests.rs");
    include!("evaluation_tests.rs");
    include!("event_stream_tests.rs");
    include!("fake_codex_tests.rs");
    include!("codex_plan_tests.rs");
    include!("pull_request_tests.rs");
    include!("repo_health_parser_tests.rs");
    include!("worker_http_tests.rs");

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[test]
    fn worker_run_state_reads_temper_odata_fields() {
        let value = json!({
            "entity_id": "wr-1",
            "status": "Queued",
            "fields": {
                "task": "Do useful work",
                "work_cycle_id": "wc-1",
                "worktree_path": "/tmp/worktree",
                "branch_name": "codex/test",
                "runner_kind": "local_codex",
                "allowed_worker_id": "mac-mini-codex-1",
                "worker_id": "mac-mini-codex-1",
                "provider_id": "local-codex",
                "required_capabilities": "local_codex,repo_write"
            }
        });

        let worker_run = worker_run_from_odata_value(value).expect("worker run should parse");

        assert_eq!(worker_run.id, "wr-1");
        assert_eq!(worker_run.status, "Queued");
        assert_eq!(worker_run.task, "Do useful work");
        assert_eq!(worker_run.work_cycle_id, "wc-1");
        assert_eq!(worker_run.worktree_path, "/tmp/worktree");
        assert_eq!(worker_run.branch_name, "codex/test");
        assert_eq!(worker_run.runner_kind, "local_codex");
        assert_eq!(worker_run.allowed_worker_id, "mac-mini-codex-1");
        assert_eq!(worker_run.worker_id, "mac-mini-codex-1");
        assert_eq!(worker_run.provider_id, "local-codex");
        assert_eq!(worker_run.required_capabilities, "local_codex,repo_write");
    }

    #[test]
    fn local_worker_claims_only_configured_local_codex_runs() {
        let configured = WorkerRunState {
            id: "wr-1".to_string(),
            status: "Queued".to_string(),
            task: "Do useful work".to_string(),
            work_cycle_id: "wc-1".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: "codex/test".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: String::new(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let unconfigured = WorkerRunState {
            id: "wr-2".to_string(),
            status: "Queued".to_string(),
            task: String::new(),
            work_cycle_id: String::new(),
            worktree_path: String::new(),
            branch_name: String::new(),
            runner_kind: String::new(),
            allowed_worker_id: String::new(),
            worker_id: String::new(),
            provider_id: String::new(),
            required_capabilities: String::new(),
        };
        let cloud = WorkerRunState {
            id: "wr-3".to_string(),
            status: "Queued".to_string(),
            task: "Cloud overflow".to_string(),
            work_cycle_id: "wc-cloud".to_string(),
            worktree_path: String::new(),
            branch_name: String::new(),
            runner_kind: "codex_cloud".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: String::new(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let no_worktree_assignment = WorkerRunState {
            id: "wr-4".to_string(),
            status: "Queued".to_string(),
            task: "Fix a Discord trace leak".to_string(),
            work_cycle_id: "wc-no-worktree".to_string(),
            worktree_path: String::new(),
            branch_name: String::new(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: String::new(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };

        assert!(worker_run_is_claimable_by_local_codex(
            &configured,
            "mac-mini-codex-1"
        ));
        assert!(!worker_run_is_claimable_by_local_codex(
            &configured,
            "other-worker"
        ));
        assert!(!worker_run_is_claimable_by_local_codex(
            &unconfigured,
            "mac-mini-codex-1"
        ));
        assert!(!worker_run_is_claimable_by_local_codex(
            &cloud,
            "mac-mini-codex-1"
        ));
        assert!(!worker_run_is_claimable_by_local_codex(
            &no_worktree_assignment,
            "mac-mini-codex-1"
        ));
    }

    #[test]
    fn local_worker_recovers_only_own_running_codex_runs() {
        let recoverable = WorkerRunState {
            id: "wr-1".to_string(),
            status: "Running".to_string(),
            task: "Resume this work".to_string(),
            work_cycle_id: "wc-1".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: "codex/test".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let other_worker = WorkerRunState {
            worker_id: "other-worker".to_string(),
            ..recoverable.clone()
        };
        let queued = WorkerRunState {
            status: "Queued".to_string(),
            ..recoverable.clone()
        };

        assert!(worker_run_is_recoverable_by_local_codex(
            &recoverable,
            "mac-mini-codex-1"
        ));
        assert!(!worker_run_is_recoverable_by_local_codex(
            &other_worker,
            "mac-mini-codex-1"
        ));
        assert!(!worker_run_is_recoverable_by_local_codex(
            &queued,
            "mac-mini-codex-1"
        ));
    }

    #[test]
    fn repo_sweep_worker_runs_are_detected_for_review_and_evaluation() {
        let worker_run = WorkerRunState {
            id: "wr-1".to_string(),
            status: "Done".to_string(),
            task: "RepoGraphSnapshot: snap-1\nWorkCycle: wc-1".to_string(),
            work_cycle_id: "wc-1".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: "codex/repo-sweep".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: String::new(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let normal_worker_run = WorkerRunState {
            id: "wr-2".to_string(),
            status: "Done".to_string(),
            task: "Fix a Discord reply bug".to_string(),
            work_cycle_id: "wc-2".to_string(),
            worktree_path: "/tmp/worktree".to_string(),
            branch_name: "codex/bugfix".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: String::new(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };

        assert!(worker_run_is_repo_sweep(&worker_run));
        assert!(!worker_run_is_repo_sweep(&normal_worker_run));
    }

    #[test]
    fn worker_command_defaults_to_run_and_accepts_doctor() {
        assert_eq!(
            parse_worker_command(Vec::<String>::new()),
            WorkerCommand::Run
        );
        assert_eq!(
            parse_worker_command(vec!["doctor".to_string()]),
            WorkerCommand::Doctor
        );
        assert_eq!(
            parse_worker_command(vec!["--doctor".to_string()]),
            WorkerCommand::Doctor
        );
        assert_eq!(
            parse_worker_command(vec!["launchd-plist".to_string()]),
            WorkerCommand::LaunchdPlist
        );
        assert_eq!(
            parse_worker_command(vec!["--launchd-plist".to_string()]),
            WorkerCommand::LaunchdPlist
        );
        assert_eq!(
            parse_worker_command(vec!["run".to_string()]),
            WorkerCommand::Run
        );
    }

    #[test]
    fn launchd_plist_renders_concrete_worker_environment() {
        let config = Config {
            temper_url: "https://temperpaw.example.test".to_string(),
            tenant: "prod".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            worker_token: Some("secret&token".to_string()),
            workspace_root: PathBuf::from("/Users/me/Development/temperpaw-worktrees"),
            repo_root: PathBuf::from("/Users/me/Development/temperpaw"),
            codex_bin: "/Users/me/.local/bin/codex".to_string(),
            max_concurrent_runs: 1,
            enable_execution: true,
            poll_on_start: false,
            codex_exec_smoke: true,
            codex_exec_timeout: Duration::from_secs(90),
        };

        let plist = render_launchd_plist(
            &config,
            Path::new("/Users/me/.local/bin/paw-codex-worker"),
            Some("cargo test -p temperpaw --test paw_patrol_foundation"),
        );

        for needle in [
            "<key>Label</key>",
            "<string>com.temperpaw.paw-codex-worker</string>",
            "<string>/Users/me/.local/bin/paw-codex-worker</string>",
            "<key>TEMPER_URL</key>",
            "<string>https://temperpaw.example.test</string>",
            "<key>WORKER_ID</key>",
            "<string>mac-mini-codex-1</string>",
            "<key>PATH</key>",
            "<string>/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/Users/openclaw/.cargo/bin</string>",
            "<key>PAW_CODEX_ENABLE_EXECUTION</key>",
            "<string>1</string>",
            "<key>PAW_CODEX_POLL_ON_START</key>",
            "<string>0</string>",
            "<key>PAW_CODEX_DOCTOR_EXEC_SMOKE</key>",
            "<string>1</string>",
            "<key>PAW_CODEX_EXEC_TIMEOUT_SECS</key>",
            "<string>90</string>",
            "<key>PAW_CODEX_WORKER_CAPABILITIES</key>",
            "<string>local_codex,repo_write,review,evaluation,datadog_query,github_query</string>",
            "<key>PAW_CODEX_WORKER_ENV_FILE</key>",
            "<string>/Users/openclaw/.config/temperpaw/paw-codex-worker.env</string>",
            "<key>PAW_CODEX_FORBIDDEN_DONE_PATHS</key>",
            "<key>PAW_CODEX_EVAL_COMMANDS</key>",
            "<string>cargo test -p temperpaw --test paw_patrol_foundation</string>",
        ] {
            assert!(plist.contains(needle), "plist should contain {needle}");
        }
        assert!(
            !plist.contains("<key>WORKER_TOKEN</key>"),
            "launchd plist should keep WORKER_TOKEN in the 0600 env file, not in launchctl-visible environment"
        );
        assert!(
            !plist.contains("<key>PAW_CODEX_INHERIT_USER_CONFIG</key>"),
            "tool profile opt-in should stay in the worker env file so launchd does not freeze a default"
        );
    }

    #[test]
    fn doctor_status_fails_only_on_failures() {
        let pass_and_warn = vec![
            DoctorCheck::pass("repo", "ok".to_string()),
            DoctorCheck::warn("token", "missing token".to_string()),
        ];
        let fail = vec![DoctorCheck::fail("odata", "not reachable".to_string())];

        assert!(!doctor_has_failures(&pass_and_warn));
        assert!(doctor_has_failures(&fail));
        assert_eq!(doctor_status_label(DoctorStatus::Pass), "pass");
        assert_eq!(doctor_status_label(DoctorStatus::Warn), "warn");
        assert_eq!(doctor_status_label(DoctorStatus::Fail), "fail");
    }

    #[tokio::test]
    async fn doctor_codex_exec_smoke_runs_fixture_when_enabled() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fake-codex.sh");
        let root = unique_temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let config = Config {
            temper_url: "http://127.0.0.1:3497".to_string(),
            tenant: "default".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            worker_token: Some("secret".to_string()),
            workspace_root: root.clone(),
            repo_root: root.clone(),
            codex_bin: fixture.display().to_string(),
            max_concurrent_runs: 1,
            enable_execution: false,
            poll_on_start: true,
            codex_exec_smoke: true,
            codex_exec_timeout: Duration::from_secs(30),
        };

        let check = check_codex_exec_smoke(&config).await;

        assert_eq!(check.name, "codex_exec_smoke");
        assert_eq!(check.status, DoctorStatus::Pass);
        assert!(
            check.detail.contains("PAW_CODEX_DOCTOR_EXEC_OK"),
            "unexpected detail: {}",
            check.detail
        );
        fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn run_codex_times_out_stuck_local_exec() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fake-codex.sh");
        let root = unique_temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let config = Config {
            temper_url: "http://127.0.0.1:3497".to_string(),
            tenant: "default".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            worker_token: Some("secret".to_string()),
            workspace_root: root.clone(),
            repo_root: root.clone(),
            codex_bin: fixture.display().to_string(),
            max_concurrent_runs: 1,
            enable_execution: true,
            poll_on_start: true,
            codex_exec_smoke: false,
            codex_exec_timeout: Duration::from_millis(100),
        };
        let worker_run = WorkerRunState {
            id: "wr-timeout".to_string(),
            status: "Running".to_string(),
            task: "PAW_FAKE_CODEX_HANG: simulate a stuck Codex child".to_string(),
            work_cycle_id: "wc-timeout".to_string(),
            worktree_path: root.display().to_string(),
            branch_name: "codex/timeout".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };

        let result = run_codex(&config, &worker_run).await;

        let error = result.expect_err("stuck codex exec should time out");
        let message = format!("{error:#}");
        assert!(
            message.contains("codex exec timed out after"),
            "unexpected timeout error: {message}"
        );
        fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn run_codex_timeout_cleans_child_process_group() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fake-codex.sh");
        let root = unique_temp_dir();
        fs::create_dir_all(&root).expect("temp dir");
        let marker = root.join("orphan-survived");
        let config = Config {
            temper_url: "http://127.0.0.1:3497".to_string(),
            tenant: "default".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            worker_token: Some("secret".to_string()),
            workspace_root: root.clone(),
            repo_root: root.clone(),
            codex_bin: fixture.display().to_string(),
            max_concurrent_runs: 1,
            enable_execution: true,
            poll_on_start: true,
            codex_exec_smoke: false,
            codex_exec_timeout: Duration::from_millis(100),
        };
        let worker_run = WorkerRunState {
            id: "wr-timeout-tree".to_string(),
            status: "Running".to_string(),
            task: format!("PAW_FAKE_CODEX_ORPHAN:{}", marker.display()),
            work_cycle_id: "wc-timeout-tree".to_string(),
            worktree_path: root.display().to_string(),
            branch_name: "codex/timeout-tree".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };

        let result = run_codex(&config, &worker_run).await;

        let error = result.expect_err("stuck codex exec should time out");
        assert!(
            format!("{error:#}").contains("codex exec timed out after"),
            "unexpected timeout error: {error:#}"
        );
        sleep(Duration::from_millis(1_500)).await;
        assert!(
            !marker.exists(),
            "timed-out codex exec must not leave descendant processes running"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn codex_review_verdict_requires_explicit_marker() {
        let approved =
            parse_codex_review_verdict("SUMMARY: Looks good\nVERDICT: approve\nLIVE_E2E: passed");
        let changes =
            parse_codex_review_verdict("VERDICT: request_changes\nSUMMARY: Missing E2E proof");
        let escalated =
            parse_codex_review_verdict("VERDICT: escalate\nSUMMARY: Cedar risk needs human");
        let unknown = parse_codex_review_verdict("Looks fine to me");

        assert_eq!(approved.action, ReviewDecisionAction::Approve);
        assert_eq!(changes.action, ReviewDecisionAction::RequestChanges);
        assert_eq!(escalated.action, ReviewDecisionAction::Escalate);
        assert_eq!(unknown.action, ReviewDecisionAction::Escalate);
        assert!(unknown.summary.contains("explicit VERDICT"));
    }

    #[test]
    fn forbidden_done_path_scan_catches_deployment_sensitive_edits() {
        let status = "\
 M dd-dashboards/temperpaw-overview.json
 M os-apps/paw-channels/wasm/transport_reconcile/Cargo.lock
?? .proofs/patrol.md
";

        let violation = forbidden_done_path_violations(
            status,
            "os-apps/paw-agent/,os-apps/paw-channels/,crates/paw-triggers/",
        );

        assert_eq!(
            violation,
            vec!["os-apps/paw-channels/wasm/transport_reconcile/Cargo.lock"]
        );
    }

    #[test]
    fn evaluation_commands_default_to_temperpaw_foundation_check() {
        let commands = evaluation_commands(None);
        assert_eq!(
            commands,
            vec!["cargo test -p temperpaw --test paw_patrol_foundation -- --nocapture"]
        );

        let configured =
            evaluation_commands(Some("cargo check -p paw-codex-worker\n git diff --check "));
        assert_eq!(
            configured,
            vec!["cargo check -p paw-codex-worker", "git diff --check"]
        );
    }

    #[test]
    fn codex_success_summary_includes_git_evidence_for_proof_packets() {
        let worker_run = WorkerRunState {
            id: "wr-proof".to_string(),
            status: "Running".to_string(),
            task: "Fix a Discord trace leak".to_string(),
            work_cycle_id: "wc-proof".to_string(),
            worktree_path: "/tmp/paw-worktree".to_string(),
            branch_name: "codex/trace-leak".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let evidence = WorktreeEvidence {
            status_short: " M crates/temperpaw/src/discord.rs\n?? docs/proofs/trace.md\n"
                .to_string(),
            diff_stat: "crates/temperpaw/src/discord.rs | 12 ++++++------".to_string(),
        };

        let summary = format_codex_success_summary(
            &worker_run,
            Path::new("/tmp/paw-worktree"),
            "implemented the fix",
            &evidence,
            None,
        );

        assert!(summary.contains("codex exec completed for WorkerRun wr-proof"));
        assert!(summary.contains("Worktree: /tmp/paw-worktree"));
        assert!(summary.contains("```git-status"));
        assert!(summary.contains(" M crates/temperpaw/src/discord.rs"));
        assert!(summary.contains("?? docs/proofs/trace.md"));
        assert!(summary.contains("```git-diff-stat"));
        assert!(summary.contains("Codex stdout"));
    }

    #[tokio::test]
    async fn codex_exec_bypasses_sandbox_for_assigned_worktree() {
        let _guard = ENV_LOCK.lock().await;
        let _datadog_mcp =
            EnvOverride::set("PAW_CODEX_ENABLE_DATADOG_MCP", OsString::from("0"));
        let args = codex_exec_args(Path::new("/tmp/paw-worktree"), "Create the proof file");
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "exec",
                "--ignore-user-config",
                "--ephemeral",
                "--dangerously-bypass-approvals-and-sandbox",
                "--cd",
                "/tmp/paw-worktree",
                "--skip-git-repo-check",
                "Create the proof file"
            ]
        );
    }

    #[tokio::test]
    async fn codex_exec_keeps_datadog_mcp_disabled_when_env_unset() {
        let _guard = ENV_LOCK.lock().await;
        let _datadog_mcp = EnvOverride::remove("PAW_CODEX_ENABLE_DATADOG_MCP");
        let _datadog_url = EnvOverride::remove("PAW_CODEX_DATADOG_MCP_URL");
        let args = codex_exec_args(Path::new("/tmp/paw-worktree"), "Create the proof file");
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "--ignore-user-config");
        assert!(!args.contains(&"-c".to_string()));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("mcp_servers.datadog.url"))
        );
    }

    #[tokio::test]
    async fn codex_exec_can_enable_datadog_mcp_without_inheriting_user_config() {
        let _guard = ENV_LOCK.lock().await;
        let _datadog_mcp =
            EnvOverride::set("PAW_CODEX_ENABLE_DATADOG_MCP", OsString::from("1"));
        let _datadog_url = EnvOverride::set(
            "PAW_CODEX_DATADOG_MCP_URL",
            OsString::from("https://mcp.datadoghq.test/mcp?toolsets=logs"),
        );
        let args = codex_exec_args(
            Path::new("/tmp/paw-worktree"),
            "Inspect Datadog evidence for the evolution direction",
        );
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(
            &args[0..4],
            &[
                "exec".to_string(),
                "--ignore-user-config".to_string(),
                "-c".to_string(),
                "mcp_servers.datadog.url=\"https://mcp.datadoghq.test/mcp?toolsets=logs\""
                    .to_string(),
            ]
        );
        assert!(args.contains(&"--ephemeral".to_string()));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
        assert_eq!(
            args.last().map(String::as_str),
            Some("Inspect Datadog evidence for the evolution direction")
        );
    }

    #[tokio::test]
    async fn codex_exec_uses_default_datadog_mcp_url_when_enabled() {
        let _guard = ENV_LOCK.lock().await;
        let _datadog_mcp =
            EnvOverride::set("PAW_CODEX_ENABLE_DATADOG_MCP", OsString::from("1"));
        let _datadog_url = EnvOverride::remove("PAW_CODEX_DATADOG_MCP_URL");
        let args = codex_exec_args(
            Path::new("/tmp/paw-worktree"),
            "Inspect Datadog evidence for the evolution direction",
        );
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(args.contains(
            &"mcp_servers.datadog.url=\"https://mcp.datadoghq.com/api/unstable/mcp-server/mcp?toolsets=all\""
                .to_string()
        ));
    }

    #[test]
    fn toml_basic_string_escapes_codex_config_values() {
        assert_eq!(
            toml_basic_string("https://mcp.example.test/a\"b\\c\n"),
            "\"https://mcp.example.test/a\\\"b\\\\c\\n\""
        );
    }

    #[test]
    fn worker_proof_text_does_not_call_assigned_worktree_current_checkout() {
        let worker_run = WorkerRunState {
            id: "wr-existing-worktree".to_string(),
            status: "Running".to_string(),
            task: "Inspect an existing assigned worktree".to_string(),
            work_cycle_id: "wc-existing-worktree".to_string(),
            worktree_path: "/tmp/paw-existing-worktree".to_string(),
            branch_name: String::new(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write".to_string(),
        };
        let evidence = WorktreeEvidence {
            status_short: String::new(),
            diff_stat: String::new(),
        };

        let summary = format_codex_success_summary(
            &worker_run,
            Path::new("/tmp/paw-existing-worktree"),
            "inspected",
            &evidence,
            None,
        );
        let review = ReviewRunState {
            status: "Requested".to_string(),
            worker_run_id: worker_run.id.clone(),
            proof_packet_id: "proof-1".to_string(),
        };
        let prompt = codex_review_prompt(&worker_run, &review);

        assert!(summary.contains("Branch: (assigned worktree without branch)"));
        assert!(prompt.contains("Branch: (assigned worktree without branch)"));
        assert!(!summary.contains("Branch: (current checkout)"));
        assert!(!prompt.contains("Branch: (current checkout)"));
    }

    #[test]
    fn review_prompt_allows_pr_ready_changes_with_deployment_pending() {
        let worker_run = WorkerRunState {
            id: "wr-dashboard".to_string(),
            status: "Done".to_string(),
            task: "Update Datadog dashboard JSON but do not deploy production.".to_string(),
            work_cycle_id: "wc-dashboard".to_string(),
            worktree_path: "/tmp/paw-dashboard".to_string(),
            branch_name: "codex/dashboard".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write,review".to_string(),
        };
        let review = ReviewRunState {
            status: "Requested".to_string(),
            worker_run_id: worker_run.id.clone(),
            proof_packet_id: "proof-dashboard".to_string(),
        };

        let prompt = codex_review_prompt(&worker_run, &review);

        assert!(prompt.contains("deployment-pending residual risk"));
        assert!(prompt.contains("Do not require production deployment"));
    }

    #[test]
    fn repo_health_review_prompt_reviews_scan_contract_not_patch_contract() {
        let worker_run = WorkerRunState {
            id: "wr-repo-scan".to_string(),
            status: "Done".to_string(),
            task: "RepoGraphSnapshot: snap-1\nWorkCycle: wc-1\nRun an agent-led repo health patrol.".to_string(),
            work_cycle_id: "wc-1".to_string(),
            worktree_path: "/tmp/paw-repo-scan".to_string(),
            branch_name: "codex/paw-repo-sweep-snap-1".to_string(),
            runner_kind: "local_codex".to_string(),
            allowed_worker_id: "mac-mini-codex-1".to_string(),
            worker_id: "mac-mini-codex-1".to_string(),
            provider_id: "local-codex".to_string(),
            required_capabilities: "local_codex,repo_write,evaluation".to_string(),
        };
        let review = ReviewRunState {
            status: "Requested".to_string(),
            worker_run_id: worker_run.id.clone(),
            proof_packet_id: "proof-1".to_string(),
        };

        let prompt = codex_review_prompt(&worker_run, &review);

        assert!(prompt.contains("repo-health Patrol scan reviewer"));
        assert!(prompt.contains("RepoGraphSnapshot.ScanComplete"));
        assert!(prompt.contains("do not require an implementation patch"));
        assert!(!prompt.contains("Inspect the git diff, changed files, tests/proofs"));
    }

    #[test]
    fn review_evaluation_and_work_cycle_state_read_temper_odata_fields() {
        let review = review_run_from_odata_value(json!({
            "entity_id": "rev-1",
            "status": "Requested",
            "fields": {
                "work_cycle_id": "wc-1",
                "worker_run_id": "wr-1",
                "proof_packet_id": "proof-1"
            }
        }))
        .expect("review run");
        let evaluation = evaluation_run_from_odata_value(json!({
            "entity_id": "eval-1",
            "status": "Queued",
            "fields": {
                "work_cycle_id": "wc-1",
                "required_checks": "[\"repo-sweep\"]"
            }
        }))
        .expect("evaluation run");
        let work_cycle = work_cycle_from_odata_value(json!({
            "entity_id": "wc-1",
            "status": "Reviewing",
            "fields": {
                "implementer_worker_run_id": "wr-1",
                "reviewer_run_id": "rev-1",
                "evaluation_run_id": "eval-1",
                "review_passed": "true"
            }
        }))
        .expect("work cycle");

        assert_eq!(review.worker_run_id, "wr-1");
        assert_eq!(evaluation.work_cycle_id, "wc-1");
        assert_eq!(work_cycle.status, "Reviewing");
        assert_eq!(work_cycle.implementer_worker_run_id, "wr-1");
        assert!(work_cycle.review_passed);
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        env::temp_dir().join(format!(
            "paw-codex-worker-test-{}-{nanos}",
            std::process::id()
        ))
    }

    struct EnvOverride {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvOverride {
        fn set(key: &'static str, value: OsString) -> Self {
            let previous = env::var_os(key);
            unsafe {
                env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = env::var_os(key);
            unsafe {
                env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvOverride {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = &self.previous {
                    env::set_var(self.key, value);
                } else {
                    env::remove_var(self.key);
                }
            }
        }
    }

    fn run_raw_git_for_test(args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            if output.stdout.is_empty() || output.stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_git_for_test(workdir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git -C {} {} failed: {}{}{}",
            workdir.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            if output.stdout.is_empty() || output.stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_git_capture_for_test(workdir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git -C {} {} failed: {}{}{}",
            workdir.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            if output.stdout.is_empty() || output.stderr.is_empty() {
                ""
            } else {
                "\n"
            },
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
