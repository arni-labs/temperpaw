//! Cloud deployment workflow for TemperPaw.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

const MODAL_BRIDGE_SECRET_NAME: &str = "temperpaw-bridge-auth";
const MODAL_BRIDGE_SECRET_KEY: &str = "BRIDGE_AUTH_TOKEN";
const MODAL_BRIDGE_APP_NAME: &str = "temperpaw-sandbox-bridge";
const DATADOG_POSTGRES_AGENT_SERVICE_NAME: &str = "datadog-postgres-agent";
const DATADOG_RUNTIME_AGENT_SERVICE_NAME: &str = "datadog-runtime-agent";
const RAILWAY_SERVICE_HEALTHCHECK_TIMEOUT_SECS: u64 = 3600;
const RAILWAY_HEALTH_POLL_INTERVAL_SECS: u64 = 10;
const RAILWAY_HEALTH_POLL_ATTEMPTS: u64 =
    RAILWAY_SERVICE_HEALTHCHECK_TIMEOUT_SECS / RAILWAY_HEALTH_POLL_INTERVAL_SECS;

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeployStorageMode {
    Postgres,
    Turso,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RailwayStorageTarget {
    Postgres,
    Turso { url: String, auth_token: String },
}

fn deploy_storage_mode_from_env() -> Result<DeployStorageMode> {
    match optional_env("TEMPERPAW_DEPLOY_STORAGE")
        .unwrap_or_else(|| "postgres".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "postgres" | "postgresql" => Ok(DeployStorageMode::Postgres),
        "turso" | "libsql" => Ok(DeployStorageMode::Turso),
        other => {
            anyhow::bail!("TEMPERPAW_DEPLOY_STORAGE must be `postgres` or `turso`; got {other:?}")
        }
    }
}

fn railway_storage_variables(target: RailwayStorageTarget) -> Vec<String> {
    match target {
        RailwayStorageTarget::Postgres => vec![
            "TEMPER_EVENT_STORE=postgres".to_string(),
            "TEMPER_PLATFORM_STORE=postgres".to_string(),
            "TEMPER_QUERY_PROJECTION_STORE=postgres".to_string(),
            "DATABASE_URL=${{Postgres.DATABASE_URL}}".to_string(),
        ],
        RailwayStorageTarget::Turso { url, auth_token } => vec![
            "TEMPER_EVENT_STORE=turso".to_string(),
            "TEMPER_PLATFORM_STORE=turso".to_string(),
            "TEMPER_QUERY_PROJECTION_STORE=turso".to_string(),
            format!("TURSO_URL={url}"),
            format!("TURSO_AUTH_TOKEN={auth_token}"),
        ],
    }
}

fn datadog_runtime_variables(
    dd_api_key: Option<String>,
    dd_app_key: Option<String>,
    dd_site: Option<String>,
) -> Vec<String> {
    let datadog_enabled = dd_api_key.is_some();
    let mut variables = Vec::new();

    if let Some(key) = dd_api_key {
        variables.push(format!("DD_API_KEY={key}"));
    }
    if let Some(key) = dd_app_key {
        variables.push(format!("DD_APP_KEY={key}"));
    }
    if let Some(site) = dd_site {
        variables.push(format!("DD_SITE={site}"));
    }

    variables.push("DD_SERVICE=temperpaw".to_string());
    variables.push("DD_ENV=prod".to_string());
    variables.push("DD_TAGS=team:temperpaw".to_string());

    if datadog_enabled {
        variables.push("TEMPER_PROFILING_ENABLED=true".to_string());
        variables.push("TEMPER_PROFILING_AUTO_UPLOAD=true".to_string());
        variables.push("TEMPER_DATADOG_RAILWAY_PROFILE=datadog-enhanced-railway".to_string());
        variables.push("DD_LLMOBS_ENABLED=true".to_string());
        variables.push("DD_LLMOBS_API_ENABLED=true".to_string());
        variables.push(
            "OTEL_RESOURCE_ATTRIBUTES=service.name=temperpaw,deployment.environment=prod,dd_llmobs_enabled=false"
                .to_string(),
        );
        variables.push(
            "OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-runtime-agent.railway.internal:4318"
                .to_string(),
        );
        variables.push("DD_AGENT_HOST=datadog-runtime-agent.railway.internal".to_string());
        variables.push("DD_TRACE_AGENT_PORT=8126".to_string());
        variables.push(
            "DD_TRACE_AGENT_URL=http://datadog-runtime-agent.railway.internal:8126".to_string(),
        );
    } else {
        variables.push("TEMPER_DATADOG_RAILWAY_PROFILE=portable-otel".to_string());
    }

    variables
}

fn datadog_runtime_agent_variables(dd_api_key: &str, dd_site: &str) -> Vec<String> {
    vec![
        format!("DD_API_KEY={dd_api_key}"),
        format!("DD_SITE={dd_site}"),
        "DD_ENV=prod".to_string(),
        "DD_SERVICE=temperpaw".to_string(),
        "DD_HOSTNAME=temperpaw-runtime-agent".to_string(),
        "DD_TAGS=team:temperpaw service:temperpaw railway_profile:datadog-enhanced".to_string(),
        "DD_APM_ENABLED=true".to_string(),
        "DD_APM_NON_LOCAL_TRAFFIC=true".to_string(),
        "DD_APM_FEATURES=enable_operation_and_resource_name_logic_v2".to_string(),
        "DD_LOGS_ENABLED=true".to_string(),
        "DD_OTLP_CONFIG_LOGS_ENABLED=true".to_string(),
        "DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_HTTP_ENDPOINT=0.0.0.0:4318".to_string(),
        "DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT=0.0.0.0:4317".to_string(),
        "DD_PROCESS_AGENT_ENABLED=true".to_string(),
    ]
}

fn datadog_postgres_agent_variables(dd_api_key: &str, dd_site: &str) -> Vec<String> {
    vec![
        format!("DD_API_KEY={dd_api_key}"),
        format!("DD_SITE={dd_site}"),
        "DD_ENV=prod".to_string(),
        "DD_HOSTNAME=temperpaw-postgres-dbm".to_string(),
        "DD_TAGS=team:temperpaw service:temperpaw".to_string(),
        "DD_APM_ENABLED=true".to_string(),
        "DD_APM_NON_LOCAL_TRAFFIC=true".to_string(),
        "DD_APM_FEATURES=enable_operation_and_resource_name_logic_v2".to_string(),
        "PGHOST=${{Postgres.PGHOST}}".to_string(),
        "PGPORT=${{Postgres.PGPORT}}".to_string(),
        "PGUSER=${{Postgres.PGUSER}}".to_string(),
        "PGPASSWORD=${{Postgres.PGPASSWORD}}".to_string(),
        "PGDATABASE=${{Postgres.PGDATABASE}}".to_string(),
    ]
}

fn datadog_postgres_agent_config() -> &'static str {
    r#"init_config:

instances:
  - dbm: true
    host: "%%env_PGHOST%%"
    port: "%%env_PGPORT%%"
    username: "%%env_PGUSER%%"
    password: "%%env_PGPASSWORD%%"
    dbname: "%%env_PGDATABASE%%"
    reported_hostname: "temperpaw-postgres"
    tags:
      - service:temperpaw
      - team:temperpaw
"#
}

fn datadog_postgres_agent_dockerfile() -> &'static str {
    "FROM datadog/agent:7.84.5\n\
     COPY postgres.yaml /etc/datadog-agent/conf.d/postgres.d/conf.yaml\n"
}

fn datadog_runtime_agent_dockerfile() -> &'static str {
    "FROM datadog/agent:7.84.5\n\
     LABEL railway_profile=datadog-enhanced\n\
     LABEL org.opencontainers.image.description=\"TemperPaw Railway runtime Datadog Agent. USM requires system-probe host capabilities. Continuous ddprof is canary-gated from the temperpaw service.\"\n"
}

// ---------------------------------------------------------------------------
// Credential cache — persists tokens/keys between deploy runs
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/share/temperpaw/deploy_cache.json")
}

fn load_cache() -> HashMap<String, String> {
    let path = cache_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &HashMap<String, String>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(cache).unwrap_or_default(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

fn cache_get(cache: &HashMap<String, String>, key: &str) -> Option<String> {
    cache.get(key).filter(|v| !v.is_empty()).cloned()
}

fn cache_set(cache: &mut HashMap<String, String>, key: &str, value: &str) {
    cache.insert(key.to_string(), value.to_string());
    save_cache(cache);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModalCredentials {
    profile: String,
    token_id: String,
    token_secret: String,
}

#[derive(Debug, Default)]
struct ModalProfileConfig {
    token_id: Option<String>,
    token_secret: Option<String>,
    active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModalBridgeConfig {
    credentials: ModalCredentials,
    bridge_url: String,
}

pub async fn run_deploy() -> Result<()> {
    cliclack::intro("Temper Paw Deploy")?;

    let mut cache = load_cache();

    cliclack::log::info("All services use free tiers — no credit card required.")?;

    cliclack::log::step("Checking prerequisites...")?;
    let storage_mode = deploy_storage_mode_from_env()?;
    ensure_or_install("railway", &install_railway)?;
    if storage_mode == DeployStorageMode::Turso {
        ensure_or_install("turso", &install_turso)?;
    }
    ensure_or_install("wrangler", &install_wrangler)?;
    ensure_or_install("modal", &install_modal)?;

    ensure_auth_railway()?;
    if storage_mode == DeployStorageMode::Turso {
        ensure_auth_turso(&mut cache)?;
    }
    ensure_auth_wrangler(&mut cache)?;

    let modal_bridge = match configure_modal_bridge() {
        Ok(config) => config,
        Err(error) => {
            return Err(error.context(
                "Failed to provision the Modal sandbox bridge. The deploy flow should own this automatically.",
            ));
        }
    };

    let owner = slugify(
        &std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "temperpaw".to_string()),
    );
    let project_name = format!("temperpaw-{owner}");
    let database_name = format!("temperpaw-{owner}");
    let bucket_name = format!("temperpaw-fs-{owner}");

    cliclack::log::step("Provisioning R2 bucket (free tier: 10 GB storage)...")?;
    create_r2_bucket_idempotent(&bucket_name)?;

    let (blob_access_key, blob_secret_key, blob_endpoint) =
        collect_r2_credentials(&bucket_name, &mut cache)?;

    cliclack::log::step("Creating Railway project (free tier: 512 MB RAM, 1 vCPU)...")?;
    create_railway_project_idempotent(&project_name)?;

    let storage_target = match storage_mode {
        DeployStorageMode::Postgres => {
            cliclack::log::step("Provisioning Railway Postgres database...")?;
            ensure_railway_postgres_database()?;
            RailwayStorageTarget::Postgres
        }
        DeployStorageMode::Turso => {
            cliclack::log::step(
                "Provisioning legacy Turso database (free tier: 9 GB, 500M rows)...",
            )?;
            create_turso_db_idempotent(&database_name)?;
            let turso_url = capture_trimmed("turso", &["db", "show", &database_name, "--url"])?;
            let turso_auth_token =
                capture_trimmed("turso", &["db", "tokens", "create", &database_name])?;
            RailwayStorageTarget::Turso {
                url: turso_url,
                auth_token: turso_auth_token,
            }
        }
    };

    let existing_temperpaw_vars = load_service_variables("temperpaw").unwrap_or_default();
    let vault_key_b64 = existing_temperpaw_vars
        .get("TEMPER_VAULT_KEY")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| {
            let generated = generate_vault_key_b64();
            let _ = cliclack::log::info(
                "Generated TEMPER_VAULT_KEY for this environment (it will be reused on later deploys).",
            );
            generated
        });
    let temper_api_key = existing_temperpaw_vars
        .get("TEMPER_API_KEY")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| {
            let generated = generate_temper_api_key();
            let _ = cliclack::log::info(
                "Generated TEMPER_API_KEY for this environment (it will be reused on later deploys).",
            );
            generated
        });

    let uses_postgres = matches!(storage_target, RailwayStorageTarget::Postgres);
    let mut variables = railway_storage_variables(storage_target);
    variables.extend([
        format!("BLOB_ENDPOINT={blob_endpoint}"),
        format!("BLOB_BUCKET={bucket_name}"),
        format!("BLOB_ACCESS_KEY={blob_access_key}"),
        format!("BLOB_SECRET_KEY={blob_secret_key}"),
        format!("TEMPER_API_KEY={temper_api_key}"),
        format!("TEMPER_VAULT_KEY={vault_key_b64}"),
    ]);

    if let Some(modal_bridge) = &modal_bridge {
        variables.push("SANDBOX_PROVIDER=modal".to_string());
        variables.push(format!(
            "MODAL_TOKEN_ID={}",
            modal_bridge.credentials.token_id
        ));
        variables.push(format!(
            "MODAL_TOKEN_SECRET={}",
            modal_bridge.credentials.token_secret
        ));
        variables.push(format!("MODAL_BRIDGE_URL={}", modal_bridge.bridge_url));
    } else {
        cliclack::log::warning(
            "Modal credentials were not found locally, so this deploy is continuing without automatic Modal sandbox provisioning.\n  Run `modal token set` once and redeploy to let TemperPaw provision and persist the bridge automatically.",
        )?;
    }

    // Datadog observability — optional
    let (dd_api_key, dd_app_key, dd_site) = prompt_datadog_config(&mut cache)?;
    variables.extend(datadog_runtime_variables(
        dd_api_key.clone(),
        dd_app_key.clone(),
        dd_site.clone(),
    ));

    let mut set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        "temperpaw".to_string(),
    ];
    set_args.extend(variables);
    run_interactive("railway", &as_str_slice(&set_args))?;

    // Get project/env IDs for all deploys
    let (project_id, env_id) = get_railway_ids()?;

    // Always deploy the OTEL collector — it auto-detects DD_API_KEY at startup.
    // With DD_API_KEY → exports to Datadog. Without → debug exporter (logs to stdout).
    // To enable Datadog later, just add DD_API_KEY to the otel-collector service in Railway.
    cliclack::log::step("Deploying OTEL collector...")?;

    let _ = Command::new("railway")
        .args(["add", "--service", "otel-collector"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Set DD vars on the collector (if available now — can be added later via Railway dashboard)
    let mut collector_vars: Vec<String> = Vec::new();
    if let Some(key) = &dd_api_key {
        collector_vars.push(format!("DD_API_KEY={key}"));
    }
    if let Some(site) = &dd_site {
        collector_vars.push(format!("DD_SITE={site}"));
    }
    let mut collector_set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        "otel-collector".to_string(),
    ];
    collector_set_args.extend(collector_vars);
    run_interactive("railway", &as_str_slice(&collector_set_args))?;

    deploy_otel_collector(&project_id, &env_id)?;

    if let Some(key) = &dd_api_key {
        deploy_datadog_runtime_agent(
            &project_id,
            &env_id,
            key,
            dd_site.as_deref().unwrap_or("datadoghq.com"),
        )?;
    } else {
        cliclack::log::info(
            "Datadog runtime agent skipped because Datadog is disabled for this deploy.",
        )?;
    }

    if uses_postgres {
        if let Some(key) = &dd_api_key {
            deploy_datadog_postgres_agent(
                &project_id,
                &env_id,
                key,
                dd_site.as_deref().unwrap_or("datadoghq.com"),
            )?;
        } else {
            cliclack::log::info(
                "Postgres DBM agent skipped because Datadog is disabled for this deploy.",
            )?;
        }
    }

    if dd_api_key.is_some() {
        cliclack::log::success("OTEL collector → Datadog ✓")?;
    } else {
        cliclack::log::info(
            "OTEL collector deployed in debug mode (no DD_API_KEY).\n  \
             To enable Datadog: add DD_API_KEY to the otel-collector service in Railway.\n  \
             The collector will auto-detect it on restart.",
        )?;
    }

    // Point temperpaw at the active OTLP route via Railway private networking.
    let (otel_endpoint, railway_profile) = if dd_api_key.is_some() {
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT=http://datadog-runtime-agent.railway.internal:4318",
            "TEMPER_DATADOG_RAILWAY_PROFILE=datadog-enhanced-railway",
        )
    } else {
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector.railway.internal:4318",
            "TEMPER_DATADOG_RAILWAY_PROFILE=portable-otel",
        )
    };
    run_interactive(
        "railway",
        &[
            "variable",
            "set",
            "-s",
            "temperpaw",
            otel_endpoint,
            "OTEL_ENABLED=true",
            railway_profile,
        ],
    )?;

    // Capture Railway metadata so the deployed server can call Railway's API from the dashboard.
    // These enable the Update / Deploy buttons in the sidebar.
    let mut meta_vars = vec![
        format!("RAILWAY_PROJECT_ID={project_id}"),
        format!("RAILWAY_ENVIRONMENT_ID={env_id}"),
    ];

    // Resolve service IDs for both services
    if let Ok(temperpaw_service_id) = resolve_railway_service_id(&project_id, &env_id, "temperpaw")
    {
        meta_vars.push(format!("RAILWAY_SERVICE_ID={temperpaw_service_id}"));
    }
    if let Ok(otel_service_id) = resolve_railway_service_id(&project_id, &env_id, "otel-collector")
    {
        meta_vars.push(format!("RAILWAY_OTEL_SERVICE_ID={otel_service_id}"));
    }
    if let Ok(runtime_agent_service_id) =
        resolve_railway_service_id(&project_id, &env_id, DATADOG_RUNTIME_AGENT_SERVICE_NAME)
    {
        meta_vars.push(format!(
            "RAILWAY_DATADOG_RUNTIME_AGENT_SERVICE_ID={runtime_agent_service_id}"
        ));
    }

    // Use the locally-authenticated Railway CLI token for API calls.
    // `railway token` was removed in newer CLI versions, so read from the config file.
    if let Some(token) = read_railway_cli_token() {
        meta_vars.push(format!("RAILWAY_TOKEN={token}"));
    } else {
        cliclack::log::warning(
            "Could not read Railway CLI token — dashboard deploy buttons will be disabled.\n  \
             You can add RAILWAY_TOKEN manually in the Railway dashboard.",
        )?;
    }

    let mut meta_set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        "temperpaw".to_string(),
    ];
    meta_set_args.extend(meta_vars);
    run_interactive("railway", &as_str_slice(&meta_set_args))?;
    cliclack::log::success("Railway metadata captured for dashboard integration")?;

    cliclack::log::step("Deploying TemperPaw...")?;
    deploy_prebuilt_image(&project_id, &env_id)?;

    let domain_output =
        capture_trimmed("railway", &["domain", "--service", "temperpaw", "--json"])?;
    let deploy_url = infer_domain(&domain_output).unwrap_or(domain_output.clone());

    cliclack::log::step("Waiting for health check...")?;
    match poll_health(&deploy_url).await {
        Ok(()) => {
            cliclack::log::success("Server is live!")?;
        }
        Err(_) => {
            anyhow::bail!(
                "Health check timed out — the build may still be running.\n  \
                 Check Railway dashboard: https://railway.com/project → {project_name}\n  \
                 Once it's live, visit: {deploy_url}/dashboard"
            );
        }
    }

    // Configure LLM provider, API key, and model via the secrets API (not Railway env vars).
    configure_llm_via_api(&deploy_url).await?;

    let dashboard_url = format!("{deploy_url}/dashboard");
    let settings_url = format!("{deploy_url}/dashboard/settings");

    cliclack::log::success(format!("Paw is live → {dashboard_url}"))?;

    let mut todo_lines: Vec<String> = Vec::new();

    if modal_bridge.is_none() {
        todo_lines.push(
            "  • Sandbox — coding agents CANNOT run code (edit files, run commands,\n    \
             execute tests) until this is set. Add either Modal\n    \
             (MODAL_TOKEN_ID + MODAL_TOKEN_SECRET, SANDBOX_PROVIDER=modal) or\n    \
             Tensorlake (TENSORLAKE_API_KEY)."
                .to_string(),
        );
    }

    todo_lines.push(
        "  • Web search — the agent CANNOT search the web until you set EXA_API_KEY.\n    \
         Web fetch (reading a known URL) still works without it."
            .to_string(),
    );
    todo_lines.push(
        "  • Discord integration — the bot WILL NOT respond in Discord until you set\n    \
         the Discord bot token, public key, and guild/channel IDs."
            .to_string(),
    );
    todo_lines.push(
        "  • Slack integration — the bot WILL NOT respond in Slack until you set the\n    \
         Slack app token, bot token, and signing secret."
            .to_string(),
    );
    todo_lines.push(
        "  • GitHub integration — the agent CANNOT read repos or open PRs until you\n    \
         set GITHUB_TOKEN."
            .to_string(),
    );

    let todo_block = todo_lines.join("\n\n");

    cliclack::log::info(format!(
        "Still to do — set these in the dashboard:\n  \
         {settings_url}\n\
         \n\
         {todo_block}"
    ))?;

    cliclack::outro(format!(
        "Open {dashboard_url} to create your account and finish setup."
    ))?;
    Ok(())
}

/// Read the Railway CLI authentication token from its config file.
/// Works with both old (`~/.railway/config.json`) and new CLI versions.
fn read_railway_cli_token() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let config_path = PathBuf::from(&home).join(".railway/config.json");
    let data = std::fs::read_to_string(config_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;
    json.get("user")
        .and_then(|u| u.get("token"))
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
}

/// Generate a base64-encoded 32-byte random key for the secrets vault.
fn generate_vault_key_b64() -> String {
    use std::io::Read;
    let mut key = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut key))
        .unwrap_or_else(|_| {
            // Fallback: use std time-based seed (not cryptographic but good enough for deploy)
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = ((seed >> (i % 16)) ^ (i as u128 * 251)) as u8;
            }
        });
    // Manual base64 encoding to avoid adding a dependency
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(44);
    for chunk in key.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Generate a hex-encoded 32-byte API key.
fn generate_temper_api_key() -> String {
    use std::io::Read;

    let mut key = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut key))
        .unwrap_or_else(|_| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = ((seed >> (i % 16)) ^ (i as u128 * 131)) as u8;
            }
        });

    let mut out = String::with_capacity(key.len() * 2);
    for byte in key {
        let hi = (byte >> 4) & 0x0f;
        let lo = byte & 0x0f;
        out.push(nibble_to_hex(hi));
        out.push(nibble_to_hex(lo));
    }
    out
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + (nibble - 10)) as char,
        _ => unreachable!("nibble must be in 0..=15"),
    }
}

fn load_service_variables(service_name: &str) -> Result<HashMap<String, String>> {
    let output = Command::new("railway")
        .args(["variables", "--json", "-s", service_name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| {
            format!("Failed to read Railway variables for service `{service_name}`")
        })?;

    if !output.status.success() {
        anyhow::bail!("Railway variables lookup failed for service `{service_name}`");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Failed to parse Railway variables JSON for `{service_name}`"))?;

    let vars = json
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    Ok(vars)
}

fn cli_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn npm_exists() -> bool {
    cli_exists("npm")
}

fn ensure_or_install(command: &str, installer: &dyn Fn() -> Result<()>) -> Result<()> {
    if cli_exists(command) {
        cliclack::log::success(format!("{command} ✓"))?;
        return Ok(());
    }

    let spinner = cliclack::spinner();
    spinner.start(format!("Installing {command}..."));
    match installer() {
        Ok(()) => {
            if cli_exists(command) {
                spinner.stop(format!("{command} installed ✓"));
                Ok(())
            } else {
                spinner.stop(format!("{command} install finished but command not found"));
                anyhow::bail!(
                    "{command} installed but not on PATH. \
                     You may need to restart your shell and try again."
                );
            }
        }
        Err(e) => {
            spinner.stop(format!("{command} install failed"));
            Err(e.context(format!("Failed to auto-install {command}")))
        }
    }
}

fn install_railway() -> Result<()> {
    if npm_exists() {
        run_install(&["npm", "install", "-g", "@railway/cli"])
    } else {
        run_shell_install("bash <(curl -fsSL cli.new)")
    }
}

fn install_turso() -> Result<()> {
    run_shell_install("curl -sSfL https://get.tur.so/install.sh | bash")
}

fn install_wrangler() -> Result<()> {
    if npm_exists() {
        run_install(&["npm", "install", "-g", "wrangler"])
    } else {
        anyhow::bail!("wrangler requires npm. Install Node.js first: https://nodejs.org")
    }
}

fn install_modal() -> Result<()> {
    if cli_exists("uv") {
        run_install(&["uv", "tool", "install", "modal"])
    } else if cli_exists("python3") {
        run_install(&["python3", "-m", "pip", "install", "--user", "modal"])
    } else if cli_exists("pip3") {
        run_install(&["pip3", "install", "--user", "modal"])
    } else {
        anyhow::bail!("modal requires either uv, python3, or pip3 to auto-install")
    }
}

fn run_install(args: &[&str]) -> Result<()> {
    let status = Command::new(args[0])
        .args(&args[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Failed to run `{}`", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("`{}` exited with {}", args.join(" "), status);
    }
    Ok(())
}

fn run_shell_install(script: &str) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Failed to run: {script}"))?;
    if !status.success() {
        anyhow::bail!("Install script failed: {script}");
    }
    Ok(())
}

fn is_cli_logged_in(command: &str, check_args: &[&str]) -> bool {
    Command::new(command)
        .args(check_args)
        .output()
        .map(|output| {
            if !output.status.success() {
                return false;
            }
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            !combined.contains("not logged in") && !combined.contains("please login")
        })
        .unwrap_or(false)
}

fn collect_r2_credentials(
    bucket_name: &str,
    cache: &mut HashMap<String, String>,
) -> Result<(String, String, String)> {
    if let (Some(ak), Some(sk), Some(ep)) = (
        cache_get(cache, "r2_access_key"),
        cache_get(cache, "r2_secret_key"),
        cache_get(cache, "r2_endpoint"),
    ) {
        cliclack::log::success("R2 credentials loaded from previous run ✓")?;
        return Ok((ak, sk, ep));
    }

    let cf_account_id = get_cloudflare_account_id();
    let r2_token_url = match &cf_account_id {
        Some(id) => format!("https://dash.cloudflare.com/{id}/r2/api-tokens/create?type=account"),
        None => "https://dash.cloudflare.com/?to=/:account/r2/api-tokens".to_string(),
    };
    cliclack::log::info(format!(
        "Create an R2 API token. Set these two values:\n  \
         Permissions: \x1b[1mObject Read & Write\x1b[0m\n  \
         Bucket: \x1b[1m{bucket_name}\x1b[0m\n  \
         Then paste the three values it gives you below."
    ))?;
    let _ = Command::new("open").arg(&r2_token_url).status();

    let blob_access_key: String = cliclack::input("R2 Access Key ID")
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Required — shown on the token page you just created")
            } else {
                Ok(())
            }
        })
        .interact()?;
    let blob_secret_key: String = cliclack::password("R2 Secret Access Key")
        .mask('•')
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Required — shown right below the Access Key ID")
            } else {
                Ok(())
            }
        })
        .interact()?;
    let blob_endpoint: String = cliclack::input("R2 endpoint URL")
        .placeholder("https://<account-id>.r2.cloudflarestorage.com")
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Required — shown on the same token page, or on the R2 bucket overview")
            } else {
                Ok(())
            }
        })
        .interact()?;

    cache_set(cache, "r2_access_key", blob_access_key.trim());
    cache_set(cache, "r2_secret_key", blob_secret_key.trim());
    cache_set(cache, "r2_endpoint", blob_endpoint.trim());

    Ok((blob_access_key, blob_secret_key, blob_endpoint))
}

fn ensure_auth_railway() -> Result<()> {
    if is_cli_logged_in("railway", &["whoami"]) {
        cliclack::log::success("railway authenticated ✓")?;
        return Ok(());
    }

    cliclack::log::info("Not logged in to Railway. Pairing with a code...")?;
    let status = Command::new("railway")
        .args(["login", "--browserless"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run railway login")?;

    if !status.success() || !is_cli_logged_in("railway", &["whoami"]) {
        anyhow::bail!("Railway login failed. Run `railway login` manually and retry.");
    }
    cliclack::log::success("railway authenticated ✓")?;
    Ok(())
}

fn ensure_auth_turso(cache: &mut HashMap<String, String>) -> Result<()> {
    if is_cli_logged_in("turso", &["auth", "whoami"]) {
        cliclack::log::success("turso authenticated ✓")?;
        return Ok(());
    }

    if let Some(cached) = cache_get(cache, "turso_api_token") {
        unsafe {
            std::env::set_var("TURSO_API_TOKEN", &cached);
        }
        if is_cli_logged_in("turso", &["auth", "whoami"]) {
            cliclack::log::success("turso authenticated (cached token) ✓")?;
            return Ok(());
        }
    }

    cliclack::log::info(
        "Turso provides the database (free tier, no credit card).\n  \
         Opening Turso — go to API Tokens, create one, and paste it below.",
    )?;
    let _ = Command::new("open").arg("https://app.turso.tech").status();

    let token: String = cliclack::password("Paste Turso API token")
        .mask('•')
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Required — paste the token you just created on turso.tech")
            } else {
                Ok(())
            }
        })
        .interact()?;

    unsafe {
        std::env::set_var("TURSO_API_TOKEN", token.trim());
    }

    if !is_cli_logged_in("turso", &["auth", "whoami"]) {
        anyhow::bail!(
            "That token didn't work. Go to https://app.turso.tech → API Tokens, create a new one, and retry."
        );
    }
    cache_set(cache, "turso_api_token", token.trim());
    cliclack::log::success("turso authenticated ✓")?;
    Ok(())
}

fn ensure_auth_wrangler(cache: &mut HashMap<String, String>) -> Result<()> {
    if is_cli_logged_in("wrangler", &["whoami"]) {
        cliclack::log::success("wrangler authenticated ✓")?;
        return Ok(());
    }

    if let Some(cached) = cache_get(cache, "cloudflare_api_token") {
        unsafe {
            std::env::set_var("CLOUDFLARE_API_TOKEN", &cached);
        }
        if is_cli_logged_in("wrangler", &["whoami"]) {
            cliclack::log::success("wrangler authenticated (cached token) ✓")?;
            return Ok(());
        }
    }

    let token_url = "https://dash.cloudflare.com/profile/api-tokens";
    cliclack::log::info(
        "Cloudflare R2 provides file storage (free tier: 10 GB, no credit card).\n  \
         Opening Cloudflare — create a Custom Token with permission:\n  \
         Account → Workers R2 Storage → Edit. Paste it below.",
    )?;
    let _ = Command::new("open").arg(token_url).status();

    let token: String = cliclack::password("Paste Cloudflare API token")
        .mask('•')
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("Required — paste the token you just created on Cloudflare")
            } else {
                Ok(())
            }
        })
        .interact()?;

    unsafe {
        std::env::set_var("CLOUDFLARE_API_TOKEN", token.trim());
    }

    if !is_cli_logged_in("wrangler", &["whoami"]) {
        anyhow::bail!(
            "That token didn't work. Make sure you selected \"Workers R2 Storage → Edit\".\n  \
             Go to {token_url}, create a new token, and retry."
        );
    }
    cache_set(cache, "cloudflare_api_token", token.trim());
    cliclack::log::success("wrangler authenticated ✓")?;
    Ok(())
}

fn configure_modal_bridge() -> Result<Option<ModalBridgeConfig>> {
    let Some(credentials) = read_modal_credentials()? else {
        return Ok(None);
    };

    cliclack::log::step("Provisioning Modal sandbox bridge...")?;
    ensure_modal_bridge_auth_secret(&credentials)?;
    let bridge_url = deploy_modal_bridge(&credentials)?;
    cliclack::log::success(format!("Modal bridge ready ✓ {bridge_url}"))?;

    Ok(Some(ModalBridgeConfig {
        credentials,
        bridge_url,
    }))
}

fn read_modal_credentials() -> Result<Option<ModalCredentials>> {
    if let (Some(token_id), Some(token_secret)) = (
        optional_env("MODAL_TOKEN_ID"),
        optional_env("MODAL_TOKEN_SECRET"),
    ) {
        return Ok(Some(ModalCredentials {
            profile: "env".to_string(),
            token_id,
            token_secret,
        }));
    }

    let config_path = modal_config_path();
    let data = match std::fs::read_to_string(&config_path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("Failed to read Modal config from {}", config_path.display())
            });
        }
    };

    read_modal_credentials_from_str(&data, std::env::var("MODAL_PROFILE").ok().as_deref())
}

fn modal_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".modal.toml")
}

fn read_modal_credentials_from_str(
    config: &str,
    profile_override: Option<&str>,
) -> Result<Option<ModalCredentials>> {
    let mut profiles: HashMap<String, ModalProfileConfig> = HashMap::new();
    let mut current_profile: Option<String> = None;

    for raw_line in config.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let profile = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            profiles.entry(profile.clone()).or_default();
            current_profile = Some(profile);
            continue;
        }

        let Some(profile_name) = current_profile.as_ref() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = parse_modal_value(value.trim());
        let profile = profiles.entry(profile_name.clone()).or_default();

        match key {
            "token_id" => profile.token_id = Some(value),
            "token_secret" => profile.token_secret = Some(value),
            "active" => profile.active = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    if profiles.is_empty() {
        return Ok(None);
    }

    let selected_profile = if let Some(profile) = profile_override {
        profile.to_string()
    } else if let Some((name, _)) = profiles.iter().find(|(_, profile)| profile.active) {
        name.clone()
    } else if profiles.contains_key("default") {
        "default".to_string()
    } else if let Some((name, _)) = profiles
        .iter()
        .find(|(_, profile)| profile.token_id.is_some() && profile.token_secret.is_some())
    {
        name.clone()
    } else {
        return Ok(None);
    };

    let profile = profiles
        .get(&selected_profile)
        .with_context(|| format!("Modal profile `{selected_profile}` was not found"))?;

    match (profile.token_id.as_ref(), profile.token_secret.as_ref()) {
        (Some(token_id), Some(token_secret)) => Ok(Some(ModalCredentials {
            profile: selected_profile,
            token_id: token_id.clone(),
            token_secret: token_secret.clone(),
        })),
        _ => Ok(None),
    }
}

fn parse_modal_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn ensure_modal_bridge_auth_secret(credentials: &ModalCredentials) -> Result<()> {
    let key_value = format!("{MODAL_BRIDGE_SECRET_KEY}={}", credentials.token_id);

    let output = Command::new("modal")
        .args([
            "secret",
            "create",
            "--force",
            MODAL_BRIDGE_SECRET_NAME,
            &key_value,
        ])
        .env("MODAL_TOKEN_ID", &credentials.token_id)
        .env("MODAL_TOKEN_SECRET", &credentials.token_secret)
        .output()
        .context("Failed to run `modal secret create`")?;

    if !output.status.success() {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        anyhow::bail!("Failed to create/update Modal bridge auth secret: {combined}");
    }

    Ok(())
}

fn deploy_modal_bridge(credentials: &ModalCredentials) -> Result<String> {
    let script_path = modal_bridge_script_path();
    let output = Command::new("modal")
        .arg("deploy")
        .arg(&script_path)
        .env("MODAL_TOKEN_ID", &credentials.token_id)
        .env("MODAL_TOKEN_SECRET", &credentials.token_secret)
        .output()
        .with_context(|| format!("Failed to run `modal deploy {}`", script_path.display()))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if !output.status.success() {
        anyhow::bail!("Modal bridge deploy failed: {combined}");
    }

    infer_modal_bridge_base_url(&combined).with_context(|| {
        format!(
            "Modal bridge deploy succeeded but the base URL could not be inferred from output:\n{combined}"
        )
    })
}

fn modal_bridge_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("os-apps/paw-agent/modal-bridge/modal_bridge.py")
}

fn infer_modal_bridge_base_url(output: &str) -> Option<String> {
    const ENDPOINT_SUFFIXES: &[&str] = &[
        "-create",
        "-health",
        "-file-read",
        "-file-write",
        "-file-delete",
        "-exec",
        "-terminate",
    ];

    for line in output.lines() {
        let trimmed = line.trim();
        let Some(start) = trimmed.find("https://") else {
            continue;
        };
        let url = &trimmed[start..];
        if !url.contains(MODAL_BRIDGE_APP_NAME) || !url.contains(".modal.run") {
            continue;
        }
        let base = url.strip_suffix(".modal.run").unwrap_or(url);
        for suffix in ENDPOINT_SUFFIXES {
            if let Some(prefix) = base.strip_suffix(suffix) {
                return Some(prefix.to_string());
            }
        }
    }

    None
}

/// Get Railway project and environment IDs from the linked project.
fn get_railway_ids() -> Result<(String, String)> {
    let status_output = Command::new("railway")
        .args(["status", "--json"])
        .output()
        .context("Failed to get Railway project status")?;
    let status_json: serde_json::Value = serde_json::from_slice(&status_output.stdout)
        .context("Failed to parse Railway status JSON")?;
    let project_id = status_json["id"]
        .as_str()
        .context("No project ID in Railway status")?
        .to_string();
    let env_id = status_json["environments"]["edges"]
        .as_array()
        .and_then(|edges| edges.first())
        .and_then(|e| e["node"]["id"].as_str())
        .context("No environment ID in Railway status")?
        .to_string();
    Ok((project_id, env_id))
}

/// Deploy the pre-built Docker image from GHCR instead of building from source.
fn deploy_prebuilt_image(project_id: &str, env_id: &str) -> Result<()> {
    let tmp = std::env::temp_dir().join("temperpaw-deploy");
    let _ = std::fs::create_dir_all(&tmp);
    std::fs::write(
        tmp.join("Dockerfile"),
        "ARG IMAGE_TAG=latest\nFROM ghcr.io/nerdsane/temperpaw:${IMAGE_TAG}\n",
    )?;
    std::fs::write(
        tmp.join("railway.toml"),
        prebuilt_railway_manifest("Dockerfile"),
    )?;

    let status = Command::new("railway")
        .args(["up", "-s", "temperpaw", "-p", project_id, "-e", env_id])
        .current_dir(&tmp)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run railway up")?;

    let _ = std::fs::remove_dir_all(&tmp);

    if !status.success() {
        anyhow::bail!("Railway deploy failed");
    }
    Ok(())
}

/// Deploy the OTEL collector as a Railway service.
/// Interactive Datadog configuration.
///
/// Asks the user if they want Datadog observability. If yes, collects
/// DD_API_KEY (required), DD_APP_KEY (optional), and DD_SITE (default: datadoghq.com).
fn prompt_datadog_config(
    cache: &mut HashMap<String, String>,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let enable: bool = cliclack::confirm("Enable Datadog observability?")
        .initial_value(false)
        .interact()?;

    if !enable {
        cliclack::log::info(
            "Datadog skipped. You can enable it later by adding DD_API_KEY\n  \
             to the otel-collector service in Railway, then adding DD_API_KEY\n  \
             TEMPER_PROFILING_ENABLED=true, and TEMPER_PROFILING_AUTO_UPLOAD=true\n  \
             to the temperpaw runtime service.",
        )?;
        return Ok((None, None, None));
    }

    let cached_api_key = cache_get(cache, "dd_api_key").unwrap_or_default();
    let api_key: String = cliclack::input("Datadog API Key")
        .placeholder("Enter your Datadog API key")
        .default_input(&cached_api_key)
        .validate(|input: &String| {
            if input.trim().is_empty() {
                Err("API key is required for Datadog")
            } else {
                Ok(())
            }
        })
        .interact()?;
    cache_set(cache, "dd_api_key", api_key.trim());

    let cached_app_key = cache_get(cache, "dd_app_key").unwrap_or_default();
    let app_key: String =
        cliclack::input("Datadog App Key (optional — for dashboard/monitor APIs)")
            .placeholder("Press Enter to skip")
            .default_input(&cached_app_key)
            .required(false)
            .interact()?;
    let app_key = if app_key.trim().is_empty() {
        None
    } else {
        cache_set(cache, "dd_app_key", app_key.trim());
        Some(app_key.trim().to_string())
    };

    let cached_site = cache_get(cache, "dd_site").unwrap_or_else(|| "datadoghq.com".to_string());
    let site: String = cliclack::select("Datadog Site")
        .initial_value(cached_site.as_str())
        .item("datadoghq.com", "datadoghq.com", "US1 (default)")
        .item("us3.datadoghq.com", "us3.datadoghq.com", "US3")
        .item("us5.datadoghq.com", "us5.datadoghq.com", "US5")
        .item("datadoghq.eu", "datadoghq.eu", "EU1")
        .item("ap1.datadoghq.com", "ap1.datadoghq.com", "AP1 — Japan")
        .item("ap2.datadoghq.com", "ap2.datadoghq.com", "AP2 — Australia")
        .item("ddog-gov.com", "ddog-gov.com", "US1-FED — GovCloud")
        .interact()?
        .to_string();
    cache_set(cache, "dd_site", &site);

    Ok((
        Some(api_key.trim().to_string()),
        app_key,
        Some(site.trim().to_string()),
    ))
}

/// Uses a dynamic entrypoint: if DD_API_KEY is set, exports to Datadog.
/// Otherwise, uses a debug exporter (traces logged to stdout).
/// Adding DD_API_KEY later via Railway dashboard auto-restarts with Datadog enabled.
/// OTEL collector YAML emitted when the service has `DD_API_KEY` set.
/// GenAI spans are copied to Datadog LLM Observability and also kept in the
/// ordinary APM trace so guest LLM/tool spans stay stitched to host-boundary
/// spans in trace search.
///
/// Includes `resourcedetection` + `resource` processors per ADR-0037:
/// without them, Railway containers emit spans with a 12-hex container id
/// as `host.name`, which makes DD APM bucket spans oddly. The detectors
/// populate real host/container metadata; the resource processor upserts
/// `service.name=temperpaw` for any OTLP signal that arrives missing its
/// resource attributes.
fn otel_datadog_config() -> &'static str {
    r#"receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318

processors:
  resourcedetection:
    detectors: [env, system]
    timeout: 5s
    override: false
  resource:
    attributes:
      - key: service.name
        value: temperpaw
        action: upsert
      - key: service.namespace
        value: temperpaw
        action: upsert
      - key: team
        value: temperpaw
        action: upsert
  transform/dbm:
    error_mode: ignore
    trace_statements:
      - context: span
        statements:
          - set(attributes["span.type"], "sql") where attributes["db.system"] != nil and attributes["span.type"] == nil
  batch:
    send_batch_size: 1000
    timeout: 5s

exporters:
  datadog:
    api:
      key: ${env:DD_API_KEY}
      site: ${env:DD_SITE}

service:
  pipelines:
    traces/apm:
      receivers: [otlp]
      processors: [resourcedetection, resource, transform/dbm, batch]
      exporters: [datadog]
    metrics:
      receivers: [otlp]
      processors: [resourcedetection, resource, batch]
      exporters: [datadog]
    logs:
      receivers: [otlp]
      processors: [resourcedetection, resource, batch]
      exporters: [datadog]
"#
}

/// Minimal OTEL collector YAML used when `DD_API_KEY` is absent — pipes
/// all signals to the debug exporter so an operator can see traces in
/// the collector logs without a Datadog account. Intentionally does not
/// include the resourcedetection/resource processors: the debug exporter
/// prints raw OTLP payloads, so enrichment would just add noise.
fn otel_debug_config() -> &'static str {
    "receivers:\n\
     \x20 otlp:\n\
     \x20   protocols:\n\
     \x20     grpc:\n\
     \x20       endpoint: 0.0.0.0:4317\n\
     \x20     http:\n\
     \x20       endpoint: 0.0.0.0:4318\n\
     \n\
     processors:\n\
     \x20 batch:\n\
     \x20   send_batch_size: 1000\n\
     \x20   timeout: 5s\n\
     \n\
     exporters:\n\
     \x20 debug:\n\
     \x20   verbosity: basic\n\
     \n\
     service:\n\
     \x20 pipelines:\n\
     \x20   traces:\n\
     \x20     receivers: [otlp]\n\
     \x20     processors: [batch]\n\
     \x20     exporters: [debug]\n\
     \x20   metrics:\n\
     \x20     receivers: [otlp]\n\
     \x20     processors: [batch]\n\
     \x20     exporters: [debug]\n\
     \x20   logs:\n\
     \x20     receivers: [otlp]\n\
     \x20     processors: [batch]\n\
     \x20     exporters: [debug]\n"
}

fn otel_collector_entrypoint() -> &'static str {
    r#"#!/bin/sh
if [ -n "$DD_API_KEY" ]; then
  echo "DD_API_KEY detected - exporting to Datadog"
  exec /otelcol-contrib \
    --feature-gates=datadog.EnableOperationAndResourceNameV2 \
    --config /etc/otelcol-contrib/otel-datadog.yaml
else
  echo "No DD_API_KEY - running in debug mode (traces logged to stdout)"
  echo "To enable Datadog: add DD_API_KEY to this service in Railway"
  exec /otelcol-contrib --config /etc/otelcol-contrib/otel-debug.yaml
fi
"#
}

fn deploy_otel_collector(project_id: &str, env_id: &str) -> Result<()> {
    let tmp = std::env::temp_dir().join("temperpaw-otel-deploy");
    let _ = std::fs::create_dir_all(&tmp);

    // Dockerfile with both config files + a startup script that picks the right one
    std::fs::write(
        tmp.join("Dockerfile"),
        "FROM otel/opentelemetry-collector-contrib:latest AS collector\n\
         \n\
         FROM alpine:3.20\n\
         \n\
         RUN apk add --no-cache ca-certificates\n\
         \n\
         COPY --from=collector /otelcol-contrib /otelcol-contrib\n\
         COPY otel-datadog.yaml /etc/otelcol-contrib/otel-datadog.yaml\n\
         COPY otel-debug.yaml /etc/otelcol-contrib/otel-debug.yaml\n\
         COPY --chmod=755 entrypoint.sh /entrypoint.sh\n\
         ENTRYPOINT [\"/entrypoint.sh\"]\n",
    )?;

    // Entrypoint: choose config based on DD_API_KEY presence
    std::fs::write(tmp.join("entrypoint.sh"), otel_collector_entrypoint())?;

    // Config with Datadog exporter — used when DD_API_KEY is present
    std::fs::write(tmp.join("otel-datadog.yaml"), otel_datadog_config())?;

    // Config with debug exporter — used when DD_API_KEY is absent
    std::fs::write(tmp.join("otel-debug.yaml"), otel_debug_config())?;

    std::fs::write(
        tmp.join("railway.toml"),
        "[build]\nbuilder = \"dockerfile\"\ndockerfilePath = \"Dockerfile\"\n\n\
         [deploy]\nrestartPolicyType = \"ON_FAILURE\"\nrestartPolicyMaxRetries = 3\n",
    )?;

    let status = Command::new("railway")
        .args(["up", "-s", "otel-collector", "-p", project_id, "-e", env_id])
        .current_dir(&tmp)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to deploy OTEL collector")?;

    let _ = std::fs::remove_dir_all(&tmp);

    if !status.success() {
        anyhow::bail!("OTEL collector deploy failed");
    }

    cliclack::log::success("OTEL collector deployed ✓")?;
    Ok(())
}

fn deploy_datadog_runtime_agent(
    project_id: &str,
    env_id: &str,
    dd_api_key: &str,
    dd_site: &str,
) -> Result<()> {
    cliclack::log::step("Deploying Datadog runtime agent...")?;

    let _ = Command::new("railway")
        .args(["add", "--service", DATADOG_RUNTIME_AGENT_SERVICE_NAME])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let mut set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        DATADOG_RUNTIME_AGENT_SERVICE_NAME.to_string(),
    ];
    set_args.extend(datadog_runtime_agent_variables(dd_api_key, dd_site));
    run_interactive("railway", &as_str_slice(&set_args))?;

    let tmp = std::env::temp_dir().join("temperpaw-datadog-runtime-agent-deploy");
    let _ = std::fs::create_dir_all(&tmp);
    std::fs::write(tmp.join("Dockerfile"), datadog_runtime_agent_dockerfile())?;
    std::fs::write(
        tmp.join("railway.toml"),
        "[build]\nbuilder = \"dockerfile\"\ndockerfilePath = \"Dockerfile\"\n\n\
         [deploy]\nrestartPolicyType = \"ON_FAILURE\"\nrestartPolicyMaxRetries = 3\n",
    )?;

    let status = Command::new("railway")
        .args([
            "up",
            "-s",
            DATADOG_RUNTIME_AGENT_SERVICE_NAME,
            "-p",
            project_id,
            "-e",
            env_id,
        ])
        .current_dir(&tmp)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to deploy Datadog runtime agent")?;

    let _ = std::fs::remove_dir_all(&tmp);

    if !status.success() {
        anyhow::bail!("Datadog runtime agent deploy failed");
    }

    cliclack::log::success("Datadog runtime agent deployed ✓")?;
    Ok(())
}

fn deploy_datadog_postgres_agent(
    project_id: &str,
    env_id: &str,
    dd_api_key: &str,
    dd_site: &str,
) -> Result<()> {
    cliclack::log::step("Deploying Datadog Postgres DBM agent...")?;

    let _ = Command::new("railway")
        .args(["add", "--service", DATADOG_POSTGRES_AGENT_SERVICE_NAME])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let mut set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        DATADOG_POSTGRES_AGENT_SERVICE_NAME.to_string(),
    ];
    set_args.extend(datadog_postgres_agent_variables(dd_api_key, dd_site));
    run_interactive("railway", &as_str_slice(&set_args))?;

    let tmp = std::env::temp_dir().join("temperpaw-datadog-postgres-agent-deploy");
    let _ = std::fs::create_dir_all(&tmp);
    std::fs::write(tmp.join("Dockerfile"), datadog_postgres_agent_dockerfile())?;
    std::fs::write(tmp.join("postgres.yaml"), datadog_postgres_agent_config())?;
    std::fs::write(
        tmp.join("railway.toml"),
        "[build]\nbuilder = \"dockerfile\"\ndockerfilePath = \"Dockerfile\"\n\n\
         [deploy]\nrestartPolicyType = \"ON_FAILURE\"\nrestartPolicyMaxRetries = 3\n",
    )?;

    let status = Command::new("railway")
        .args([
            "up",
            "-s",
            DATADOG_POSTGRES_AGENT_SERVICE_NAME,
            "-p",
            project_id,
            "-e",
            env_id,
        ])
        .current_dir(&tmp)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to deploy Datadog Postgres DBM agent")?;

    let _ = std::fs::remove_dir_all(&tmp);

    if !status.success() {
        anyhow::bail!("Datadog Postgres DBM agent deploy failed");
    }

    cliclack::log::success("Datadog Postgres DBM agent deployed ✓")?;
    Ok(())
}

fn create_railway_project_idempotent(project_name: &str) -> Result<()> {
    let linked = Command::new("railway")
        .args(["status", "--json"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if linked {
        cliclack::log::success("Railway project already linked ✓")?;
    } else {
        let status = Command::new("railway")
            .args(["init", "--name", project_name])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run railway init")?;

        if !status.success() {
            let link_status = Command::new("railway")
                .args(["link"])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status()
                .context("Failed to run railway link")?;

            if !link_status.success() {
                anyhow::bail!("Could not create or link to Railway project \"{project_name}\".");
            }
        }
    }

    let _ = Command::new("railway")
        .args(["add", "--service", "temperpaw"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(())
}

fn ensure_railway_postgres_database() -> Result<()> {
    if load_service_variables("Postgres").is_ok() {
        cliclack::log::success("Railway Postgres already provisioned ✓")?;
        return Ok(());
    }

    let status = Command::new("railway")
        .args(["add", "--database", "postgres"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to provision Railway Postgres")?;

    if !status.success() {
        anyhow::bail!(
            "Railway Postgres provisioning failed. Add a Postgres database to the Railway project and retry."
        );
    }

    cliclack::log::success("Railway Postgres provisioned ✓")?;
    Ok(())
}

fn get_cloudflare_account_id() -> Option<String> {
    let output = Command::new("wrangler").args(["whoami"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        for word in line.split_whitespace() {
            let clean = word.trim_matches('│').trim_matches('|').trim();
            if clean.len() == 32 && clean.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(clean.to_string());
            }
        }
    }
    None
}

fn create_turso_db_idempotent(database_name: &str) -> Result<()> {
    let output = Command::new("turso")
        .args(["db", "create", database_name, "--wait"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("Failed to run turso db create")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");
    if combined.contains("already exists") {
        cliclack::log::success(format!("Database {database_name} already exists ✓"))?;
        return Ok(());
    }

    anyhow::bail!("Failed to create Turso database: {combined}");
}

fn create_r2_bucket_idempotent(bucket_name: &str) -> Result<()> {
    let output = Command::new("wrangler")
        .args(["r2", "bucket", "create", bucket_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("Failed to run wrangler r2 bucket create")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("already exists") || stderr.contains("10004") {
        cliclack::log::success(format!("Bucket {bucket_name} already exists ✓"))?;
        return Ok(());
    }

    anyhow::bail!("Failed to create R2 bucket: {stderr}");
}

fn run_interactive(command: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .with_context(|| format!("Failed to run `{command}`"))?;

    if !status.success() {
        anyhow::bail!("`{command}` failed (exit {status})");
    }

    Ok(())
}

fn capture_trimmed(command: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("Failed to run `{command} {}`", args.join(" ")))?;

    if !output.status.success() {
        anyhow::bail!("`{command} {}` failed", args.join(" "));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        anyhow::bail!("`{command} {}` returned no output", args.join(" "));
    }
    Ok(text.lines().last().unwrap_or_default().trim().to_string())
}

fn infer_domain(raw: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|json| {
            json.get("domain")
                .and_then(|value| value.as_str())
                .map(|domain| format!("https://{domain}"))
        })
}

fn prebuilt_railway_manifest(dockerfile_path: &str) -> String {
    format!(
        "[build]\nbuilder = \"dockerfile\"\ndockerfilePath = \"{dockerfile_path}\"\n\n\
         [deploy]\nhealthcheckPath = \"/readyz\"\nhealthcheckTimeout = {RAILWAY_SERVICE_HEALTHCHECK_TIMEOUT_SECS}\n\
         restartPolicyType = \"ON_FAILURE\"\nrestartPolicyMaxRetries = 3\n",
    )
}

async fn poll_health(base_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("{base_url}/readyz");

    // Cold boots can take a long time while Railway waits for the service to
    // finish restoring state and reconciling OS apps, so keep the local poll
    // window aligned with Railway's health check timeout.
    for _ in 0..RAILWAY_HEALTH_POLL_ATTEMPTS {
        if let Ok(response) = client.get(&health_url).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(RAILWAY_HEALTH_POLL_INTERVAL_SECS)).await;
    }

    anyhow::bail!("Timed out waiting for {health_url}");
}

fn slugify(input: &str) -> String {
    let slug: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-')
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Resolve a Railway service ID by name using the Railway CLI.
fn resolve_railway_service_id(
    project_id: &str,
    _env_id: &str,
    service_name: &str,
) -> Result<String> {
    // `railway service --json` lists services in the linked project.
    // We look for one with the matching name.
    let output = Command::new("railway")
        .args(["service", "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .context("Failed to run railway service --json")?;

    if !output.status.success() {
        // Fallback: try the status endpoint which includes service info
        let status_output = Command::new("railway")
            .args(["status", "--json"])
            .output()
            .context("Failed to get Railway status")?;
        let status_json: serde_json::Value = serde_json::from_slice(&status_output.stdout)?;

        // Look in services array
        if let Some(services) = status_json["services"]["edges"].as_array() {
            for service in services {
                let name = service["node"]["name"].as_str().unwrap_or_default();
                if name == service_name
                    && let Some(id) = service["node"]["id"].as_str()
                {
                    return Ok(id.to_string());
                }
            }
        }
        anyhow::bail!("Service {service_name} not found in project {project_id}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    // The output could be an array of services or an object with edges
    if let Some(arr) = json.as_array() {
        for svc in arr {
            let name = svc["name"].as_str().unwrap_or_default();
            if name == service_name
                && let Some(id) = svc["id"].as_str()
            {
                return Ok(id.to_string());
            }
        }
    } else if let Some(edges) = json["edges"].as_array() {
        for edge in edges {
            let name = edge["node"]["name"].as_str().unwrap_or_default();
            if name == service_name
                && let Some(id) = edge["node"]["id"].as_str()
            {
                return Ok(id.to_string());
            }
        }
    }

    anyhow::bail!("Service {service_name} not found in project {project_id}")
}

// ---------------------------------------------------------------------------
// LLM configuration — provider, key, and model selection via secrets API
// ---------------------------------------------------------------------------

/// Interactive LLM setup: provider → API key → model (fetched from provider) → POST to secrets.
async fn configure_llm_via_api(deploy_url: &str) -> Result<()> {
    cliclack::log::step("Configure LLM provider")?;

    let providers = [
        ("anthropic", "Anthropic (Claude)"),
        ("openai", "OpenAI"),
        ("openai_codex", "OpenAI Codex (ChatGPT Plus)"),
        ("openrouter", "OpenRouter (multi-provider)"),
    ];

    let provider: String = cliclack::select("Which LLM provider?")
        .items(
            &providers
                .iter()
                .map(|(val, label)| (val.to_string(), *label, ""))
                .collect::<Vec<_>>(),
        )
        .interact()?;

    let (secret_key, api_key) = collect_provider_key(&provider)?;

    // Fetch available models from the provider API, with a spinner.
    let spinner = cliclack::spinner();
    spinner.start("Fetching available models...");
    let models = fetch_models(&provider, &api_key).await;
    spinner.stop("Models loaded");

    let model = if models.is_empty() {
        cliclack::log::warning("Could not fetch models — enter one manually.")?;
        let m: String = cliclack::input("Model name")
            .validate(|input: &String| {
                if input.trim().is_empty() {
                    Err("Required — choose a model from your provider account")
                } else {
                    Ok(())
                }
            })
            .interact()?;
        m
    } else {
        let items: Vec<(String, String, String)> = models
            .iter()
            .map(|m| (m.clone(), m.clone(), String::new()))
            .collect();
        cliclack::select("Which model?").items(&items).interact()?
    };

    // POST all three settings to the deployed server's secrets API.
    let client = reqwest::Client::new();
    post_secret(&client, deploy_url, "llm_provider", &provider).await?;
    post_secret(&client, deploy_url, &secret_key, &api_key).await?;
    post_secret(&client, deploy_url, "llm_model", &model).await?;

    cliclack::log::success(format!("LLM configured: {provider} / {model}"))?;
    Ok(())
}

/// Prompt the user for the API key/token for the chosen provider.
/// Returns (secret_key_name, api_key_value).
fn collect_provider_key(provider: &str) -> Result<(String, String)> {
    match provider {
        "anthropic" => {
            // Check env first
            if let Some(key) = optional_env("ANTHROPIC_API_KEY") {
                cliclack::log::success("Anthropic API key detected from environment")?;
                return Ok(("anthropic_api_key".into(), key));
            }
            let key: String = cliclack::password("Anthropic API Key (sk-ant-...)")
                .mask('*')
                .validate(|input: &String| {
                    if input.trim().is_empty() {
                        Err("Required — get one at console.anthropic.com")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;
            Ok(("anthropic_api_key".into(), key.trim().to_string()))
        }
        "openai" => {
            if let Some(key) = optional_env("OPENAI_API_KEY") {
                cliclack::log::success("OpenAI API key detected from environment")?;
                return Ok(("openai_api_key".into(), key));
            }
            let key: String = cliclack::password("OpenAI API Key (sk-...)")
                .mask('*')
                .validate(|input: &String| {
                    if input.trim().is_empty() {
                        Err("Required — get one at platform.openai.com/api-keys")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;
            Ok(("openai_api_key".into(), key.trim().to_string()))
        }
        "openai_codex" => {
            if let Some(token) = optional_env("OPENAI_CODEX_TOKEN") {
                cliclack::log::warning(
                    "Using legacy OPENAI_CODEX_TOKEN fallback. Prefer dashboard device-code login.",
                )?;
                return Ok(("openai_codex_access_token".into(), token));
            }
            anyhow::bail!(
                "OpenAI Codex subscription auth is configured after deploy in the dashboard with device-code login"
            )
        }
        "openrouter" => {
            if let Some(key) = optional_env("OPENROUTER_API_KEY") {
                cliclack::log::success("OpenRouter API key detected from environment")?;
                return Ok(("openrouter_api_key".into(), key));
            }
            let key: String = cliclack::password("OpenRouter API Key (sk-or-...)")
                .mask('*')
                .validate(|input: &String| {
                    if input.trim().is_empty() {
                        Err("Required — get one at openrouter.ai/keys")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;
            Ok(("openrouter_api_key".into(), key.trim().to_string()))
        }
        _ => anyhow::bail!("Unknown provider: {provider}"),
    }
}

/// Fetch model list from the provider's API. Returns an empty vec on failure.
async fn fetch_models(provider: &str, api_key: &str) -> Vec<String> {
    match provider {
        "anthropic" => fetch_anthropic_models(api_key).await.unwrap_or_default(),
        "openai" => fetch_openai_models(api_key).await.unwrap_or_default(),
        "openai_codex" => read_codex_models_cache(),
        "openrouter" => fetch_openrouter_models(api_key).await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn fetch_anthropic_models(api_key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let mut models: Vec<String> = json["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    // Sort so newest/most capable appear first (Claude models sort well lexically in reverse)
    models.sort();
    models.reverse();
    Ok(models)
}

async fn fetch_openai_models(api_key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let mut models: Vec<String> = json["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    // Filter to chat-capable models (gpt-*, o1-*, o3-*, o4-*)
                    if id.starts_with("gpt-")
                        || id.starts_with("o1-")
                        || id.starts_with("o3-")
                        || id.starts_with("o4-")
                    {
                        Some(id.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.reverse();
    Ok(models)
}

/// Read the Codex CLI's cached model list from ~/.codex/models_cache.json.
/// This file is maintained by the Codex CLI and is entitlement-aware (shows
/// only models available to the user's ChatGPT plan).
fn read_codex_models_cache() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let cache_path = std::path::Path::new(&home).join(".codex/models_cache.json");
    let data = match std::fs::read_to_string(&cache_path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_str(&data) {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    json["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let slug = m["slug"].as_str()?;
                    // Only show models visible in the picker
                    let vis = m["visibility"].as_str().unwrap_or("list");
                    if vis == "list" || vis == "default" {
                        Some(slug.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_openrouter_models(api_key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://openrouter.ai/api/v1/models")
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    let json: serde_json::Value = resp.json().await?;
    let mut models: Vec<String> = json["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    // Only include well-known providers to keep the list manageable
                    if id.starts_with("anthropic/")
                        || id.starts_with("openai/")
                        || id.starts_with("google/")
                        || id.starts_with("meta-llama/")
                        || id.starts_with("mistralai/")
                        || id.starts_with("deepseek/")
                    {
                        Some(id.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Ok(models)
}

/// POST a single secret to the deployed server.
async fn post_secret(
    client: &reqwest::Client,
    deploy_url: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    let url = format!("{deploy_url}/paw/setup/secrets");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "key": key, "value": value }))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .with_context(|| format!("Failed to POST secret {key}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("POST {key} failed ({status}): {body}");
    }
    Ok(())
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn as_str_slice(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}
