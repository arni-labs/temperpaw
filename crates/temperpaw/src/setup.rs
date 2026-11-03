//! Interactive setup for Temper Paw.
//!
//! Two phases:
//! - Phase A (pre-boot): API key + messaging platform config
//! - Phase B (post-boot): User interview + LLM soul generation → writes to TemperFS via OData
//!
//! Phase B requires the server to be running so it can write the personalized
//! soul directly to Temper entities — no intermediate files on disk.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::config::Config;
use crate::setup_llm::{self, GeneratedSoul, LlmProvider, UserInterview};
use axum::http::HeaderMap;

/// Result of Phase A (config setup).
pub struct SetupResult {
    pub api_key: Option<String>,
    pub provider: Option<String>,
    pub discord_bot_token: Option<String>,
    pub discord_public_key: Option<String>,
    pub discord_guild_id: Option<String>,
    pub discord_feed_channel_id: Option<String>,
    pub discord_forum_channel_id: Option<String>,
    pub slack_app_token: Option<String>,
    pub slack_bot_token: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SetupRequestAuth {
    pub cookie: Option<String>,
    pub authorization: Option<String>,
}

impl SetupRequestAuth {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            cookie: headers
                .get(axum::http::header::COOKIE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        }
    }

    pub fn from_cookie(cookie: impl Into<String>) -> Self {
        Self {
            cookie: Some(cookie.into()),
            authorization: None,
        }
    }

    pub fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = if let Some(cookie) = &self.cookie {
            request.header(reqwest::header::COOKIE, cookie)
        } else {
            request
        };

        if let Some(authorization) = &self.authorization {
            request.header(reqwest::header::AUTHORIZATION, authorization)
        } else {
            request
        }
    }
}

fn has_llm_credentials(config: &Config) -> bool {
    config.anthropic_api_key.is_some()
        || config.openrouter_api_key.is_some()
        || config.huggingface_api_key.is_some()
        || config.fireworks_api_key.is_some()
        || config.sakana_fugu_api_key.is_some()
        || config.openai_compatible_api_key.is_some()
        || config.openai_compatible_api_url.is_some()
        || config.local_openai_api_url.is_some()
        || config.openai_api_key.is_some()
        || config.openai_codex_token.is_some()
}

/// Returns `true` if config setup should run automatically during boot.
///
/// Messaging setup stays opt-in via `temperpaw setup`; we only block startup when
/// no usable LLM credentials are available.
pub fn needs_setup(_data_dir: &Path, config: &Config) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    !has_llm_credentials(config)
}

/// Phase A: Collect API key and messaging config (runs pre-boot).
pub async fn run_setup_config(config: &Config) -> anyhow::Result<SetupResult> {
    let has_api_key = has_llm_credentials(config);

    let mut result = SetupResult {
        api_key: None,
        provider: None,
        discord_bot_token: None,
        discord_public_key: None,
        discord_guild_id: None,
        discord_feed_channel_id: None,
        discord_forum_channel_id: None,
        slack_app_token: None,
        slack_bot_token: None,
    };

    cliclack::intro("Temper Paw")?;

    // ─── API Key ───

    if has_api_key {
        cliclack::log::success("API key configured")?;
    } else {
        let provider: &str = cliclack::select("Which AI provider do you use?")
            .item(
                "anthropic",
                "Anthropic",
                "Pay-per-token · console.anthropic.com → API Keys",
            )
            .item(
                "openai",
                "OpenAI (API key)",
                "Pay-per-token · platform.openai.com/api-keys",
            )
            .item(
                "openai_codex",
                "OpenAI (Codex subscription)",
                "Included in ChatGPT Plus/Pro · dashboard device-code login",
            )
            .item(
                "openrouter",
                "OpenRouter",
                "Pay-per-token · openrouter.ai/keys",
            )
            .interact()?;

        if provider == "openai_codex" {
            cliclack::log::info(
                "OpenAI Codex subscription auth is configured in the dashboard with device-code login.",
            )?;
            result.provider = Some(provider.to_string());
        } else {
            let prompt = match provider {
                "anthropic" => "Anthropic API key",
                "openai" => "OpenAI API key",
                "openrouter" => "OpenRouter API key",
                _ => "API key",
            };

            let key: String = cliclack::password(prompt)
                .mask('•')
                .validate(|input: &String| {
                    if input.trim().is_empty() {
                        Err("Required")
                    } else {
                        Ok(())
                    }
                })
                .interact()?;

            let key = key.trim().to_string();
            result.api_key = Some(key);
            result.provider = Some(provider.to_string());
        }
    }

    cliclack::log::info(
        "Connect Discord, Slack, and other integrations in the dashboard after boot.",
    )?;
    Ok(result)
}

/// Phase B: User interview + LLM soul generation (runs post-boot, writes to TemperFS via OData).
///
/// `api_port` is the local server port for OData calls.
/// `api_key` is the LLM provider key for generating the soul.
/// `provider` is "anthropic", "openrouter", or "openai".
pub async fn run_setup_soul_interview(
    api_key: &str,
    provider_name: &str,
    model_name: &str,
) -> anyhow::Result<Option<GeneratedSoul>> {
    if !std::io::stdin().is_terminal() || api_key.is_empty() {
        return Ok(None);
    }

    cliclack::log::step("Let's make Paw yours.")?;

    let name: String = cliclack::input("What's your name?")
        .placeholder("your name")
        .interact()?;

    let about: String = cliclack::input("Tell Paw about yourself.")
        .placeholder("what you do, what you're working on, what you care about")
        .interact()?;

    let ideal: String = cliclack::input("What kind of Paw do you want?")
        .placeholder("how they think, how they talk, what makes them great to work with")
        .interact()?;

    let provider = LlmProvider::detect(api_key, provider_name, model_name)?;

    // Round 2: LLM-generated follow-ups
    let mut followup_answers = Vec::new();
    let followup_spinner = cliclack::spinner();
    followup_spinner.start("Thinking of something to ask you...");

    match setup_llm::generate_followup_questions(&provider, &name, &about, &ideal).await {
        Ok(questions) => {
            followup_spinner.stop("Got it.");
            for question in questions {
                let answer: String = cliclack::input(&question).placeholder("").interact()?;
                if !answer.is_empty() {
                    followup_answers.push((question, answer));
                }
            }
        }
        Err(e) => {
            followup_spinner.stop(format!("Skipped follow-ups: {e}"));
        }
    }

    // Generate soul
    let interview = UserInterview {
        name: name.clone(),
        about_you: about,
        ideal_paw: ideal,
        followup_answers,
    };

    let soul_spinner = cliclack::spinner();
    soul_spinner.start("Generating your Paw...");

    match setup_llm::generate_personalized_soul(&provider, &interview).await {
        Ok(mut generated) => {
            soul_spinner.stop("Done.");

            // Preview + iteration loop
            loop {
                let choice: &str = cliclack::select(format!(
                    "Here's what Paw will be like:\n\n  \"{}\"\n",
                    generated.summary
                ))
                .item("accept", "Looks good", "")
                .item("adjust", "Almost — let me adjust", "")
                .item("redo", "Start over", "")
                .interact()?;

                match choice {
                    "accept" => break,
                    "adjust" => {
                        let feedback: String = cliclack::input("What should change?")
                            .placeholder("describe what to adjust")
                            .interact()?;

                        let refine_spinner = cliclack::spinner();
                        refine_spinner.start("Regenerating...");
                        match setup_llm::refine_soul(
                            &provider,
                            &interview,
                            &generated.summary,
                            &feedback,
                        )
                        .await
                        {
                            Ok(refined) => {
                                refine_spinner.stop("Done.");
                                generated = refined;
                            }
                            Err(e) => {
                                refine_spinner.stop(format!("Failed: {e}"));
                                break;
                            }
                        }
                    }
                    "redo" => {
                        let redo_spinner = cliclack::spinner();
                        redo_spinner.start("Regenerating from scratch...");
                        match setup_llm::generate_personalized_soul(&provider, &interview).await {
                            Ok(fresh) => {
                                redo_spinner.stop("Done.");
                                generated = fresh;
                            }
                            Err(e) => {
                                redo_spinner.stop(format!("Failed: {e}"));
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }
            Ok(Some(generated))
        }
        Err(e) => {
            soul_spinner.stop(format!("Soul generation failed: {e}"));
            cliclack::log::warning(
                "Using default Paw soul. Run `cargo run -- setup` later to personalize.",
            )?;
            Ok(None)
        }
    }
}

pub async fn run_setup_soul(
    api_port: u16,
    api_key: &str,
    provider_name: &str,
    model_name: &str,
    tenant: &str,
    auth: SetupRequestAuth,
) -> anyhow::Result<()> {
    let base = format!("http://127.0.0.1:{api_port}");
    let client = reqwest::Client::new();

    if let Some(generated) = run_setup_soul_interview(api_key, provider_name, model_name).await? {
        let save_spinner = cliclack::spinner();
        save_spinner.start("Saving soul to Temper...");
        match save_soul_to_temper(&client, &base, tenant, &generated, &auth).await {
            Ok(()) => {
                save_spinner.stop("Soul saved.");
            }
            Err(e) => {
                save_spinner.stop(format!("Failed to save: {e}"));
                cliclack::log::warning(
                    "Soul generation succeeded but couldn't save to Temper. Run `cargo run -- setup` to retry.",
                )?;
            }
        }
    }

    cliclack::outro("Paw is ready.")?;
    Ok(())
}

fn entity_field_str<'a>(entity: &'a serde_json::Value, field_names: &[&str]) -> Option<&'a str> {
    field_names.iter().find_map(|field_name| {
        entity["fields"][*field_name]
            .as_str()
            .or_else(|| entity[*field_name].as_str())
    })
}

async fn resolve_paw_soul_entity(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    auth: &SetupRequestAuth,
) -> anyhow::Result<serde_json::Value> {
    let agent_url = format!("{base}/tdata/Agents?$filter=name eq 'Paw' and Status eq 'Active'");
    let agent_response: serde_json::Value = auth
        .apply(client.get(&agent_url))
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin")
        .send()
        .await?
        .json()
        .await?;

    if let Some(agent) = agent_response["value"]
        .as_array()
        .and_then(|items| items.first())
        && let Some(soul_id) = entity_field_str(agent, &["soul_id", "SoulId"])
    {
        let soul_url = format!("{base}/tdata/Souls('{soul_id}')");
        let soul_response: serde_json::Value = auth
            .apply(client.get(&soul_url))
            .header("x-tenant-id", tenant)
            .header("x-temper-principal-kind", "admin")
            .send()
            .await?
            .json()
            .await?;

        if entity_field_str(&soul_response, &["ContentFileId", "content_file_id"]).is_some() {
            return Ok(soul_response);
        }
    }

    for filter in ["Name eq 'Paw'", "name eq 'paw'"] {
        let soul_url = format!("{base}/tdata/Souls?$filter={filter}");
        let soul_response: serde_json::Value = auth
            .apply(client.get(&soul_url))
            .header("x-tenant-id", tenant)
            .header("x-temper-principal-kind", "admin")
            .send()
            .await?
            .json()
            .await?;

        if let Some(soul) = soul_response["value"]
            .as_array()
            .and_then(|items| items.first())
        {
            return Ok(soul.clone());
        }
    }

    anyhow::bail!("Paw Soul entity not found");
}

pub(crate) async fn load_paw_soul_content(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    auth: &SetupRequestAuth,
) -> anyhow::Result<(String, String)> {
    let soul = resolve_paw_soul_entity(client, base, tenant, auth).await?;
    let file_id = entity_field_str(&soul, &["ContentFileId", "content_file_id"])
        .ok_or_else(|| anyhow::anyhow!("Paw Soul has no ContentFileId"))?;

    let content = auth
        .apply(client.get(format!("{base}/tdata/Files('{file_id}')/$value")))
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin")
        .send()
        .await?
        .text()
        .await?;

    let summary = content
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .unwrap_or("Paw is ready, but not yet personalized.")
        .trim()
        .to_string();

    Ok((summary, content))
}

pub(crate) fn default_paw_soul_content() -> anyhow::Result<String> {
    let paw_agent_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../os-apps/paw-agent/agents/paw");

    let soul_md = std::fs::read_to_string(paw_agent_dir.join("SOUL.md"))
        .context("Failed to read default Paw SOUL.md")?;
    let style_md = std::fs::read_to_string(paw_agent_dir.join("STYLE.md"))
        .context("Failed to read default Paw STYLE.md")?;
    let agent_md = std::fs::read_to_string(paw_agent_dir.join("AGENT.md"))
        .context("Failed to read default Paw AGENT.md")?;

    Ok(format!("{soul_md}\n\n{style_md}\n\n{agent_md}"))
}

pub(crate) fn generated_paw_soul_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let generated_dir = Path::new(&home).join(".local/share/temperpaw/generated");
    vec![
        generated_dir.join("paw-soul.md"),
        generated_dir.join("paw-style.md"),
        generated_dir.join("user.md"),
    ]
}

pub(crate) fn has_local_personalized_paw_soul() -> bool {
    generated_paw_soul_paths().iter().any(|path| path.exists())
}

/// Write the generated soul content to the existing Paw Soul entity in TemperFS.
pub(crate) async fn save_soul_to_temper(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    soul: &GeneratedSoul,
    auth: &SetupRequestAuth,
) -> anyhow::Result<()> {
    let paw = resolve_paw_soul_entity(client, base, tenant, auth).await?;
    let file_id = entity_field_str(&paw, &["ContentFileId", "content_file_id"])
        .ok_or_else(|| anyhow::anyhow!("Paw Soul has no ContentFileId"))?;

    // Concatenate soul + style + user + the default AGENT.md (operational instructions)
    let agent_md =
        std::fs::read_to_string("os-apps/paw-agent/agents/paw/AGENT.md").unwrap_or_default();

    let full_content = format!(
        "{}\n\n{}\n\n{}\n\n{}",
        soul.soul_md, soul.style_md, soul.user_md, agent_md
    );

    // Upload to TemperFS via PUT $value
    let upload_url = format!("{base}/tdata/Files('{file_id}')/$value");
    let resp = auth
        .apply(client.put(&upload_url))
        .header("x-tenant-id", tenant)
        .header("x-temper-principal-kind", "admin")
        .header("content-type", "text/markdown")
        .body(full_content)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Upload failed ({status}): {body}");
    }

    Ok(())
}

/// Merge Phase A results into config.
pub fn merge_setup_into_config(config: &mut Config, setup: SetupResult) {
    if let Some(key) = setup.api_key {
        match setup.provider.as_deref() {
            Some("openai") => {
                if config.openai_api_key.is_none() {
                    config.openai_api_key = Some(key);
                }
            }
            Some("openai_codex") => {
                if config.openai_codex_token.is_none() {
                    config.openai_codex_token = Some(key);
                }
            }
            Some("openrouter") => {
                if config.openrouter_api_key.is_none() {
                    config.openrouter_api_key = Some(key);
                }
            }
            _ => {
                if config.anthropic_api_key.is_none() {
                    config.anthropic_api_key = Some(key);
                }
            }
        }
    }
    if let Some(provider) = setup.provider {
        config.llm_provider = Some(provider);
    }
    if let Some(token) = setup.discord_bot_token
        && config.discord_bot_token.is_none()
    {
        config.discord_bot_token = Some(token);
    }
    if let Some(public_key) = setup.discord_public_key
        && config.discord_public_key.is_none()
    {
        config.discord_public_key = Some(public_key);
    }
    if let Some(id) = setup.discord_guild_id
        && config.discord_guild_id.is_none()
    {
        config.discord_guild_id = Some(id);
    }
    if let Some(id) = setup.discord_feed_channel_id
        && config.discord_feed_channel_id.is_none()
    {
        config.discord_feed_channel_id = Some(id);
    }
    if let Some(id) = setup.discord_forum_channel_id
        && config.discord_forum_channel_id.is_none()
    {
        config.discord_forum_channel_id = Some(id);
    }
    if let Some(token) = setup.slack_app_token
        && config.slack_app_token.is_none()
    {
        config.slack_app_token = Some(token);
    }
    if let Some(token) = setup.slack_bot_token
        && config.slack_bot_token.is_none()
    {
        config.slack_bot_token = Some(token);
    }
}

/// Print a diagnostic report.
pub fn run_doctor(data_dir: &Path, config: &Config) {
    let vault_key_path = data_dir.join("vault.key");
    let db_path = data_dir.join("paw.db");

    println!();
    println!("  Temper Paw Doctor");
    println!();

    if data_dir.exists() {
        println!("  \u{2713} Data: {}", data_dir.display());
    } else {
        println!("  \u{2717} Data directory missing");
    }

    if vault_key_path.exists() {
        println!("  \u{2713} Vault key");
    } else if config.vault_key.is_some() {
        println!("  \u{2713} Vault key (env var)");
    } else {
        println!("  \u{2717} Vault key (generated on boot)");
    }

    if db_path.exists() {
        let size = std::fs::metadata(&db_path)
            .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
            .unwrap_or_else(|_| "?".into());
        println!("  \u{2713} Database ({size})");
    } else {
        println!("  ~ Database (created on boot)");
    }

    println!();

    if has_llm_credentials(config) {
        println!("  \u{2713} API key");
    } else {
        println!("  \u{2717} API key — run `cargo run -- setup`");
    }

    if config.discord_bot_token.is_some() {
        println!("  \u{2713} Discord bot token");
        if config.discord_public_key.is_some() {
            println!("  \u{2713} Discord public key");
        } else {
            println!("  ~ Discord public key (auto-fetched from bot token when available)");
        }
        if let Some(base_url) = config.public_base_url.as_ref() {
            println!(
                "  \u{2713} Discord interactions URL: {}/discord/interaction",
                base_url.trim_end_matches('/')
            );
        } else if crate::transport_manager::ngrok_available(&config.ngrok_bin) {
            println!(
                "  \u{2713} Discord interactions URL: auto-tunnel via {} on local runs",
                config.ngrok_bin
            );
        } else {
            println!(
                "  \u{2717} Discord interactions URL — set PUBLIC_BASE_URL or install/configure {}",
                config.ngrok_bin
            );
        }
    } else {
        println!("  \u{2717} Discord — run `cargo run -- setup`");
    }

    if config.slack_app_token.is_some() && config.slack_bot_token.is_some() {
        println!("  \u{2713} Slack");
    } else {
        println!("  ~ Slack");
    }

    println!();
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use serde_json::json;

    use super::{GeneratedSoul, SetupRequestAuth, save_soul_to_temper};

    #[tokio::test]
    async fn save_soul_to_temper_forwards_cookie_auth() {
        #[derive(Clone, Default)]
        struct SeenRequests {
            cookies: Arc<Mutex<Vec<String>>>,
            bodies: Arc<Mutex<Vec<String>>>,
        }

        async fn handler(
            State(state): State<SeenRequests>,
            headers: HeaderMap,
            request: Request<Body>,
        ) -> impl IntoResponse {
            if let Some(cookie) = headers.get("cookie").and_then(|value| value.to_str().ok()) {
                state.cookies.lock().unwrap().push(cookie.to_string());
            }

            match (request.method(), request.uri().path()) {
                (&Method::GET, "/tdata/Agents") => (
                    StatusCode::OK,
                    axum::Json(json!({
                        "value": [{
                            "fields": {
                                "soul_id": "soul-1"
                            }
                        }]
                    })),
                )
                    .into_response(),
                (&Method::GET, "/tdata/Souls('soul-1')") => (
                    StatusCode::OK,
                    axum::Json(json!({
                        "fields": {
                            "ContentFileId": "file-1"
                        }
                    })),
                )
                    .into_response(),
                (&Method::GET, "/tdata/Souls") => (
                    StatusCode::OK,
                    axum::Json(json!({
                        "value": [{
                            "fields": {
                                "ContentFileId": "file-1"
                            }
                        }]
                    })),
                )
                    .into_response(),
                (&Method::PUT, "/tdata/Files('file-1')/$value") => {
                    let body = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                    state
                        .bodies
                        .lock()
                        .unwrap()
                        .push(String::from_utf8(body.to_vec()).unwrap());
                    StatusCode::OK.into_response()
                }
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }

        let seen = SeenRequests::default();
        let app = Router::new()
            .fallback(any(handler))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let soul = GeneratedSoul {
            summary: "Thoughtful collaborator".to_string(),
            soul_md: "# Soul".to_string(),
            style_md: "# Style".to_string(),
            user_md: "# User".to_string(),
        };

        save_soul_to_temper(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            "default",
            &soul,
            &SetupRequestAuth::from_cookie("paw_session=test-cookie"),
        )
        .await
        .unwrap();

        assert_eq!(
            seen.cookies.lock().unwrap().as_slice(),
            [
                "paw_session=test-cookie",
                "paw_session=test-cookie",
                "paw_session=test-cookie"
            ]
        );
        assert!(
            seen.bodies
                .lock()
                .unwrap()
                .first()
                .map(|body| body.contains("# Soul")
                    && body.contains("# Style")
                    && body.contains("# User"))
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn save_soul_to_temper_falls_back_to_named_paw_soul() {
        async fn handler(request: Request<Body>) -> impl IntoResponse {
            match (
                request.method(),
                request.uri().path(),
                request.uri().query(),
            ) {
                (&Method::GET, "/tdata/Agents", _) => (
                    StatusCode::OK,
                    axum::Json(json!({
                        "value": [{
                            "fields": {}
                        }]
                    })),
                )
                    .into_response(),
                (&Method::GET, "/tdata/Souls", Some(query))
                    if query.contains("Name%20eq%20%27Paw%27") =>
                {
                    (
                        StatusCode::OK,
                        axum::Json(json!({
                            "value": [{
                                "fields": {
                                    "ContentFileId": "file-1"
                                }
                            }]
                        })),
                    )
                        .into_response()
                }
                (&Method::GET, "/tdata/Souls", _) => (
                    StatusCode::OK,
                    axum::Json(json!({
                        "value": []
                    })),
                )
                    .into_response(),
                (&Method::PUT, "/tdata/Files('file-1')/$value", _) => {
                    StatusCode::OK.into_response()
                }
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }

        let app = Router::new().fallback(any(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let soul = GeneratedSoul {
            summary: "Thoughtful collaborator".to_string(),
            soul_md: "# Soul".to_string(),
            style_md: "# Style".to_string(),
            user_md: "# User".to_string(),
        };

        save_soul_to_temper(
            &reqwest::Client::new(),
            &format!("http://{addr}"),
            "default",
            &soul,
            &SetupRequestAuth::default(),
        )
        .await
        .unwrap();
    }

    #[test]
    fn default_paw_soul_content_includes_base_documents() {
        let content = super::default_paw_soul_content().expect("default soul content");

        assert!(content.contains("I don't pick leads from a roster."));
        assert!(content.contains("`temper.search_history`"));
    }
}
