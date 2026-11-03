use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use futures_util::StreamExt;
use repo_health::{
    extract_repo_sweep_snapshot_id, parse_repo_health_agent_output, repo_health_agent_prompt,
    repo_sweep_summary_markdown,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

mod repo_health;

#[tokio::main]
async fn main() -> Result<()> {
    load_worker_env_file()?;
    let _otel_guard = init_worker_observability();

    let command = parse_worker_command(env::args().skip(1));
    let config = Config::from_env()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    if command == WorkerCommand::Doctor {
        return run_doctor(&client, &config).await;
    }
    if command == WorkerCommand::LaunchdPlist {
        let worker_bin = launchd_worker_binary_path();
        let eval_commands = env::var("PAW_CODEX_EVAL_COMMANDS").ok();
        println!(
            "{}",
            render_launchd_plist(&config, &worker_bin, eval_commands.as_deref())
        );
        return Ok(());
    }
    if let WorkerCommand::DirectedEvolutionStartEpisode { contract_path } = &command {
        let result = run_directed_evolution_human_episode_command(
            &client,
            &config,
            contract_path.as_deref(),
        )
        .await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    validate_dsf_worker_binding(&config)?;
    info!(
        worker_id = %config.worker_id,
        temper_url = %config.temper_url,
        tenant = %config.tenant,
        max_concurrent_runs = config.max_concurrent_runs,
        enable_execution = config.enable_execution,
        worker_capabilities = %worker_capabilities().join(","),
        "paw-codex-worker starting"
    );
    if let Err(error) = report_worker_heartbeat(&client, &config).await {
        warn!(%error, "failed to report WorkerAgent heartbeat");
    }

    if config.poll_on_start {
        reconcile_dsf_model_sources(&client, &config).await?;
        recover_boot_running_runs(&client, &config).await?;
        claim_boot_queued_runs(&client, &config).await?;
        claim_boot_requested_review_runs(&client, &config).await?;
        claim_boot_queued_evaluation_runs(&client, &config).await?;
        claim_boot_queued_directed_evolution_work_items(&client, &config).await?;
    }

    loop {
        if let Err(error) = report_worker_heartbeat(&client, &config).await {
            debug!(%error, "failed to report WorkerAgent heartbeat");
        }
        if config.poll_on_start {
            reconcile_dsf_model_sources(&client, &config).await?;
            recover_boot_running_runs(&client, &config).await?;
            claim_boot_queued_runs(&client, &config).await?;
            claim_boot_requested_review_runs(&client, &config).await?;
            claim_boot_queued_evaluation_runs(&client, &config).await?;
            claim_boot_queued_directed_evolution_work_items(&client, &config).await?;
        }
        match watch_events(&client, &config).await {
            Ok(()) => {}
            Err(error) => warn!(%error, "Temper event stream disconnected"),
        }
        sleep(Duration::from_secs(1)).await;
    }
}

include!("worker_types.rs");
include!("boot_watch.rs");
include!("doctor.rs");
include!("event_loop.rs");
include!("temper_api.rs");
include!("datadog_patrol.rs");
include!("github_patrol.rs");
include!("dsf_model_patrol.rs");
include!("dsf_model_investigation.rs");
include!("daily_brief.rs");
include!("cli.rs");
include!("doctor_report.rs");
include!("launchd.rs");
include!("doctor_helpers.rs");
include!("pull_request.rs");
include!("codex_plan.rs");
include!("code_evaluation.rs");
include!("execution.rs");
include!("directed_evolution.rs");
include!("http_headers.rs");
include!("tests.rs");
include!("daily_brief_tests.rs");

fn init_worker_observability() -> Option<temper_observe::otel::OtelGuard> {
    if env::var_os("RUST_LOG").is_none() {
        unsafe {
            env::set_var("RUST_LOG", "paw_codex_worker=info,info");
        }
    }
    if env::var_os("DD_ENV").is_none() {
        unsafe {
            env::set_var("DD_ENV", "local");
        }
    }
    temper_observe::otel::init_observability("temperpaw")
}
