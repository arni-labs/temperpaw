//! Cloud deployment workflow for TemperPaw.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::Config;

// ---------------------------------------------------------------------------
// Credential cache — persists tokens/keys between deploy runs
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join(".local/share/temperpaw/deploy_cache.json")
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
    let _ = std::fs::write(&path, serde_json::to_string_pretty(cache).unwrap_or_default());
    // Owner-only read/write — same as ~/.aws/credentials
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

pub async fn run_deploy(config: Config, with_datadog: bool) -> Result<()> {
    cliclack::intro("Temper Paw Deploy")?;

    let mut cache = load_cache();

    cliclack::log::info("All services use free tiers — no credit card required.")?;

    cliclack::log::step("Checking prerequisites...")?;
    ensure_or_install("railway", &install_railway)?;
    ensure_or_install("wrangler", &install_wrangler)?;

    ensure_auth_railway()?;
    ensure_auth_wrangler(&mut cache)?;

    let owner = slugify(
        &std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "temperpaw".to_string()),
    );
    let project_name = format!("temperpaw-{owner}");
    let bucket_name = format!("temperpaw-fs-{owner}");
    let public_artifact_bucket_name = format!("temperpaw-public-artifacts-{owner}");

    cliclack::log::step("Provisioning R2 bucket (free tier: 10 GB storage)...")?;
    create_r2_bucket_idempotent(&bucket_name)?;
    create_r2_bucket_idempotent(&public_artifact_bucket_name)?;

    // Use cached R2 credentials if available, otherwise prompt
    let (blob_access_key, blob_secret_key, blob_endpoint) =
        collect_r2_credentials(&bucket_name, &mut cache)?;
    let public_artifact_base_url =
        collect_public_artifact_base_url(&public_artifact_bucket_name, &mut cache)?;

    cliclack::log::step("Creating Railway project (free tier: 512 MB RAM, 1 vCPU)...")?;
    create_railway_project_idempotent(&project_name)?;

    cliclack::log::step("Provisioning Railway Postgres database...")?;
    let _ = Command::new("railway")
        .args(["add", "--database", "postgres"])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    let mut variables = vec![
        "TEMPER_EVENT_STORE=postgres".to_string(),
        "TEMPER_PLATFORM_STORE=postgres".to_string(),
        "TEMPER_QUERY_PROJECTION_STORE=postgres".to_string(),
        "DATABASE_URL=${{Postgres.DATABASE_URL}}".to_string(),
        format!("BLOB_ENDPOINT={blob_endpoint}"),
        format!("BLOB_BUCKET={bucket_name}"),
        format!("BLOB_ACCESS_KEY={blob_access_key}"),
        format!("BLOB_SECRET_KEY={blob_secret_key}"),
        format!("PUBLISHED_BLOB_ENDPOINT={blob_endpoint}"),
        format!("PUBLISHED_BLOB_BUCKET={public_artifact_bucket_name}"),
        format!("PUBLISHED_BLOB_ACCESS_KEY={blob_access_key}"),
        format!("PUBLISHED_BLOB_SECRET_KEY={blob_secret_key}"),
    ];
    if let Some(public_artifact_base_url) = public_artifact_base_url {
        variables.push(format!(
            "PUBLISHED_BLOB_PUBLIC_BASE_URL={public_artifact_base_url}"
        ));
    }

    if let Some(dd_api_key) = config.dd_api_key.clone() {
        variables.push(format!("DD_API_KEY={dd_api_key}"));
    }
    if let Some(dd_app_key) = config.dd_app_key.clone() {
        variables.push(format!("DD_APP_KEY={dd_app_key}"));
    }
    variables.push(format!("DD_SITE={}", config.dd_site));

    let mut set_args = vec![
        "variable".to_string(),
        "set".to_string(),
        "-s".to_string(),
        "temperpaw".to_string(),
    ];
    set_args.extend(variables);
    run_interactive("railway", &as_str_slice(&set_args))?;

    if with_datadog {
        let _ = run_interactive(
            "railway",
            &[
                "add",
                "--service",
                "otel-collector",
                "--image",
                "otel/opentelemetry-collector-contrib:latest",
            ],
        );
    }

    cliclack::log::step("Deploying TemperPaw...")?;
    deploy_prebuilt_image()?;

    let domain_output = capture_trimmed("railway", &["domain", "--service", "temperpaw", "--json"])?;
    let deploy_url = infer_domain(&domain_output).unwrap_or(domain_output.clone());

    cliclack::log::step("Waiting for health check...")?;
    match poll_health(&deploy_url).await {
        Ok(()) => {
            cliclack::outro(format!(
                "Paw is live → {deploy_url}/dashboard\n  \
                 Open the dashboard to create your account and finish setup."
            ))?;
        }
        Err(_) => {
            anyhow::bail!(
                "Health check timed out — the build may still be running.\n  \
                 Check Railway dashboard: https://railway.com/project → {project_name}\n  \
                 Once it's live, visit: {deploy_url}/dashboard"
            );
        }
    }
    Ok(())
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
        // Shell installer fallback
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
        anyhow::bail!(
            "wrangler requires npm. Install Node.js first: https://nodejs.org"
        )
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

/// Collect R2 credentials — use cache if available, otherwise prompt.
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

    // Need to prompt — open the token creation page
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

    // Cache for next run
    cache_set(cache, "r2_access_key", blob_access_key.trim());
    cache_set(cache, "r2_secret_key", blob_secret_key.trim());
    cache_set(cache, "r2_endpoint", blob_endpoint.trim());

    Ok((blob_access_key, blob_secret_key, blob_endpoint))
}

fn collect_public_artifact_base_url(
    bucket_name: &str,
    cache: &mut HashMap<String, String>,
) -> Result<Option<String>> {
    let cached = cache_get(cache, "published_blob_public_base_url");
    cliclack::log::info(format!(
        "Public artifacts use a separate R2 bucket ({bucket_name}). \
         Configure an R2 public development URL or custom domain, then paste its base URL. \
         Leave blank to set PUBLISHED_BLOB_PUBLIC_BASE_URL later in Railway."
    ))?;
    let mut prompt = cliclack::input("Public artifact base URL")
        .placeholder("https://assets.example.com")
        .required(false);
    if let Some(cached) = cached.as_deref() {
        prompt = prompt.default_input(cached);
    }
    let public_base_url: String = prompt.interact()?;
    let public_base_url = public_base_url.trim().trim_end_matches('/').to_string();
    if public_base_url.is_empty() {
        return Ok(None);
    }

    cache_set(cache, "published_blob_public_base_url", &public_base_url);
    Ok(Some(public_base_url))
}

/// Railway: check existing session, fall back to `railway login --browserless`
/// which prints a pairing code — no browser callback needed.
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

/// Turso (database): check existing session, try cached token, fall back to token paste.
fn ensure_auth_turso(cache: &mut HashMap<String, String>) -> Result<()> {
    if is_cli_logged_in("turso", &["auth", "whoami"]) {
        cliclack::log::success("turso authenticated ✓")?;
        return Ok(());
    }

    // Try cached token first
    if let Some(cached) = cache_get(cache, "turso_api_token") {
        unsafe { std::env::set_var("TURSO_API_TOKEN", &cached); }
        if is_cli_logged_in("turso", &["auth", "whoami"]) {
            cliclack::log::success("turso authenticated (cached token) ✓")?;
            return Ok(());
        }
    }

    cliclack::log::info(
        "Turso provides the database (free tier, no credit card).\n  \
         Opening Turso — go to API Tokens, create one, and paste it below."
    )?;
    let _ = Command::new("open")
        .arg("https://app.turso.tech")
        .status();

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

    unsafe { std::env::set_var("TURSO_API_TOKEN", token.trim()); }

    if !is_cli_logged_in("turso", &["auth", "whoami"]) {
        anyhow::bail!(
            "That token didn't work. Go to https://app.turso.tech → API Tokens, create a new one, and retry."
        );
    }
    cache_set(cache, "turso_api_token", token.trim());
    cliclack::log::success("turso authenticated ✓")?;
    Ok(())
}

/// Cloudflare (file storage): check existing session, try cached token, fall back to token paste.
fn ensure_auth_wrangler(cache: &mut HashMap<String, String>) -> Result<()> {
    if is_cli_logged_in("wrangler", &["whoami"]) {
        cliclack::log::success("wrangler authenticated ✓")?;
        return Ok(());
    }

    // Try cached token first
    if let Some(cached) = cache_get(cache, "cloudflare_api_token") {
        unsafe { std::env::set_var("CLOUDFLARE_API_TOKEN", &cached); }
        if is_cli_logged_in("wrangler", &["whoami"]) {
            cliclack::log::success("wrangler authenticated (cached token) ✓")?;
            return Ok(());
        }
    }

    let token_url = "https://dash.cloudflare.com/profile/api-tokens";
    cliclack::log::info(
        "Cloudflare R2 provides file storage (free tier: 10 GB, no credit card).\n  \
         Opening Cloudflare — create a Custom Token with permission:\n  \
         Account → Workers R2 Storage → Edit. Paste it below."
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

    unsafe { std::env::set_var("CLOUDFLARE_API_TOKEN", token.trim()); }

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

/// Deploy the pre-built Docker image from GHCR instead of building from source.
/// Creates a temp dir with a one-line Dockerfile and runs `railway up`.
/// Reads the project ID from the linked Railway project so the temp dir doesn't
/// need its own `.railway` link.
fn deploy_prebuilt_image() -> Result<()> {
    // Get the project ID from the linked project in the current directory
    let status_output = Command::new("railway")
        .args(["status", "--json"])
        .output()
        .context("Failed to get Railway project status")?;
    let status_json: serde_json::Value =
        serde_json::from_slice(&status_output.stdout)
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

    let image = "ghcr.io/nerdsane/temperpaw:latest";
    let tmp = std::env::temp_dir().join("temperpaw-deploy");
    let _ = std::fs::create_dir_all(&tmp);
    std::fs::write(
        tmp.join("Dockerfile"),
        format!("FROM {image}\n"),
    )?;
    std::fs::write(
        tmp.join("railway.toml"),
        "[build]\nbuilder = \"dockerfile\"\ndockerfilePath = \"Dockerfile\"\n\n\
         [deploy]\nhealthcheckPath = \"/healthz\"\nhealthcheckTimeout = 60\n\
         restartPolicyType = \"ON_FAILURE\"\nrestartPolicyMaxRetries = 3\n",
    )?;

    let status = Command::new("railway")
        .args(["up", "-s", "temperpaw", "-p", &project_id, "-e", &env_id])
        .current_dir(&tmp)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .context("Failed to run railway up")?;

    // Clean up temp dir
    let _ = std::fs::remove_dir_all(&tmp);

    if !status.success() {
        anyhow::bail!("Railway deploy failed");
    }
    Ok(())
}

/// Create or reuse a Railway project, and ensure the "temperpaw" service exists.
fn create_railway_project_idempotent(project_name: &str) -> Result<()> {
    // Check if we're already linked to a project
    let linked = Command::new("railway")
        .args(["status", "--json"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if linked {
        cliclack::log::success("Railway project already linked ✓")?;
    } else {
        // Try to create — if it fails because it exists, link to it instead
        let status = Command::new("railway")
            .args(["init", "--name", project_name])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run railway init")?;

        if !status.success() {
            // Project may exist — try linking to it
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

    // Ensure the "temperpaw" service exists (ignore errors — it may already exist)
    let _ = Command::new("railway")
        .args(["add", "--service", "temperpaw"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(())
}

/// Try to extract the Cloudflare account ID from wrangler whoami.
fn get_cloudflare_account_id() -> Option<String> {
    let output = Command::new("wrangler")
        .args(["whoami"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // wrangler whoami outputs a table with Account ID as a 32-char hex string
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

/// Create a Turso database, treating "already exists" as success.
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

/// Create an R2 bucket, treating "already exists" as success.
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

/// Run a command with full terminal access (stdin/stdout/stderr inherited).
fn run_interactive(command: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .with_context(|| format!("Failed to run `{command} {}`", args.join(" ")))?;

    if !status.success() {
        anyhow::bail!("`{command} {}` failed (exit {})", args.join(" "), status);
    }

    Ok(())
}

/// Run a command and capture its stdout. Stderr goes to the terminal.
fn run_checked(command: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .with_context(|| format!("Failed to run `{command} {}`", args.join(" ")))?;

    if !output.status.success() {
        anyhow::bail!("`{command} {}` failed", args.join(" "));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn capture_trimmed(command: &str, args: &[&str]) -> Result<String> {
    let output = run_checked(command, args)?;
    if output.is_empty() {
        anyhow::bail!("`{command} {}` returned no output", args.join(" "));
    }
    Ok(output.lines().last().unwrap_or_default().trim().to_string())
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

async fn poll_health(base_url: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("{base_url}/readyz");

    // First build can take 5-10 minutes (Rust compile on Railway).
    // Poll for up to 15 minutes.
    for _ in 0..90 {
        if let Ok(response) = client.get(&health_url).send().await {
            if response.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
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

fn as_str_slice(values: &[String]) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

