//! REST API for Temper Paw setup, secrets, transport management, and agent creation.
//!
//! Mounted at `/paw/` to avoid conflicts with Temper's `/api/` tenant routes.
//! All endpoints are also usable by external agents via HTTP.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs};

use anyhow::{Context, Result, anyhow};
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use opentelemetry::trace::{Span as _, SpanKind, Status, Tracer as _};
use opentelemetry::{KeyValue, global};
use serde::{Deserialize, Serialize};
use temper_platform::PlatformState;
use temper_platform::genesis_install::GenesisRegistryInstallRequest;
use temper_runtime::tenant::TenantId;
use temper_server::request_context::AgentContext;
use temper_server::state::DispatchExtOptions;

use crate::setup::{
    SetupRequestAuth, default_paw_soul_content, has_local_personalized_paw_soul,
    load_paw_soul_content, save_soul_to_temper,
};
use crate::setup_llm::{
    GeneratedSoul, LlmProvider, UserInterview, generate_personalized_soul, refine_soul,
};
use crate::storage::PawStorage;
use crate::transport_manager::{
    DiscordConnectParams, SlackConnectParams, TransportManager, TransportStatus,
};

const DEFAULT_SETUP_AGENT_TOOLS_ENABLED: &str = "temper_create,temper_get,temper_list,temper_action,temper_patch,temper_submit_specs,temper_show_spec,temper_specs,temper_upload_wasm,temper_get_trajectories,temper_get_insights,temper_get_decisions,temper_poll_decision,temper_approve_decision,temper_deny_decision,temper_submit_policy,temper_list_policies,temper_get_policy,temper_update_policy,temper_delete_policy,temper_search_apps,temper_install_app,temper_publish_app,temper_update_app,temper_list_apps,temper_spawn_session,temper_list_sessions,temper_abort_session,temper_steer_session,temper_save_memory,temper_recall_memory,temper_write,temper_write_many,temper_read,temper_run_coding_agent,temper_get_secret,temper_datadog_query,temper_railway,temper_vercel,temper_web_search,temper_web_fetch,temper_image_generate,read,write,edit,bash";
pub(crate) const DISCORD_TRANSPORT_CONNECTION_ID: &str = "transport-discord";
const OPENAI_CODEX_AUTH_ENTITY_ID: &str = "openai-codex-auth";
const OPENAI_CODEX_AUTH_ENTITY_TYPE: &str = "OpenAICodexAuth";
const OPENAI_CODEX_ACCESS_TOKEN: &str = "openai_codex_access_token";
const OPENAI_CODEX_REFRESH_TOKEN: &str = "openai_codex_refresh_token";
const OPENAI_CODEX_EXPIRES_AT_MS: &str = "openai_codex_expires_at_ms";
const OPENAI_CODEX_ACCOUNT_ID: &str = "openai_codex_account_id";
const RAILWAY_GRAPHQL_URL: &str = "https://backboard.railway.com/graphql/v2";
const DATADOG_RUNTIME_AGENT_SERVICE_NAME: &str = "datadog-runtime-agent";
const DATADOG_RUNTIME_AGENT_IMAGE: &str = "datadog/agent:7";
const DATADOG_RUNTIME_AGENT_HOST: &str = "datadog-runtime-agent.railway.internal";
const DEFAULT_GENESIS_REGISTRY_URL: &str = "https://genesis-production-164d.up.railway.app";

/// Shared state for the setup API.
#[derive(Clone)]
pub struct SetupApiState {
    pub platform: PlatformState,
    pub storage: PawStorage,
    pub transport_manager: Arc<TransportManager>,
    pub tenant: String,
    pub agents_dir: PathBuf,
    pub base_url: String,
    pub build_version: String,
    pub build_sha: String,
}

/// Whitelisted secret key names that can be set via the API.
fn allowed_secret_keys() -> HashSet<&'static str> {
    [
        "anthropic_api_key",
        "openai_api_key",
        "openai_codex_access_token",
        "openai_codex_refresh_token",
        "openai_codex_expires_at_ms",
        "openai_codex_account_id",
        "openai_codex_token",
        "openrouter_api_key",
        "openrouter_api_url",
        "huggingface_api_key",
        "hf_token",
        "huggingface_api_url",
        "fireworks_api_key",
        "fireworks_api_url",
        "sakana_fugu_api_key",
        "sakana_fugu_api_url",
        "openai_compatible_api_key",
        "openai_compatible_api_url",
        "openai_compatible_headers_json",
        "local_openai_api_url",
        "discord_bot_token",
        "discord_public_key",
        "discord_guild_id",
        "discord_feed_channel_id",
        "discord_forum_channel_id",
        "slack_app_token",
        "slack_bot_token",
        "slack_signing_secret",
        "github_token",
        "genesis_token",
        "exa_api_key",
        "tensorlake_api_key",
        "temper_api_key",
        "llm_provider",
        "llm_model",
        "sandbox_provider",
        "modal_token_id",
        "modal_token_secret",
        "modal_bridge_url",
        // DD_* and railway_* are infrastructure config managed via Railway env vars,
        // not dashboard secrets. They're set by `temperpaw deploy` and changed in Railway.
        "railway_project_id",
        "railway_environment_id",
        "railway_otel_service_id",
        "railway_datadog_runtime_agent_service_id",
        "railway_service_id",
    ]
    .into_iter()
    .collect()
}

fn validate_setup_secret_key(key: &str) -> Result<(), String> {
    if allowed_secret_keys().contains(key) {
        return Ok(());
    }
    if key.is_empty() {
        return Err("Secret key cannot be empty".to_string());
    }
    if key.trim() != key {
        return Err("Secret key cannot contain leading or trailing whitespace".to_string());
    }
    if key.len() > 128 {
        return Err("Secret key must be 128 characters or fewer".to_string());
    }
    if key.contains("..") {
        return Err("Secret key cannot contain '..'".to_string());
    }
    if key.starts_with('.') || key.starts_with('-') {
        return Err("Secret key cannot start with '.' or '-'".to_string());
    }
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(
            "Secret key may only contain ASCII letters, numbers, underscores, dashes, dots, and colons"
                .to_string(),
        );
    }
    Ok(())
}

/// Metadata for a known secret key — used by the dashboard to render templates.
#[derive(Serialize)]
struct SecretSchema {
    key: &'static str,
    category: &'static str,
    label: &'static str,
    required: bool,
    description: &'static str,
}

fn secrets_schema() -> Vec<SecretSchema> {
    vec![
        SecretSchema {
            key: "anthropic_api_key",
            category: "llm",
            label: "Anthropic API Key",
            required: false,
            description: "Claude models — console.anthropic.com",
        },
        SecretSchema {
            key: "openai_api_key",
            category: "llm",
            label: "OpenAI API Key",
            required: false,
            description: "GPT models — platform.openai.com/api-keys",
        },
        SecretSchema {
            key: "openai_codex_access_token",
            category: "llm",
            label: "OpenAI Codex Access Token",
            required: false,
            description: "TemperPaw-managed ChatGPT/Codex subscription OAuth access token",
        },
        SecretSchema {
            key: "openrouter_api_key",
            category: "llm",
            label: "OpenRouter API Key",
            required: false,
            description: "Multi-provider routing — openrouter.ai/keys",
        },
        SecretSchema {
            key: "openrouter_api_url",
            category: "llm",
            label: "OpenRouter API URL",
            required: false,
            description: "Optional endpoint override; defaults to OpenRouter chat completions",
        },
        SecretSchema {
            key: "huggingface_api_key",
            category: "llm",
            label: "Hugging Face API Key",
            required: false,
            description: "Hugging Face Inference Providers — supports open-weight hosted models",
        },
        SecretSchema {
            key: "huggingface_api_url",
            category: "llm",
            label: "Hugging Face API URL",
            required: false,
            description: "Optional endpoint override; defaults to Hugging Face router chat completions",
        },
        SecretSchema {
            key: "fireworks_api_key",
            category: "llm",
            label: "Fireworks API Key",
            required: false,
            description: "Fireworks AI hosted open-weight models",
        },
        SecretSchema {
            key: "fireworks_api_url",
            category: "llm",
            label: "Fireworks API URL",
            required: false,
            description: "Optional endpoint override; defaults to Fireworks chat completions",
        },
        SecretSchema {
            key: "sakana_fugu_api_key",
            category: "llm",
            label: "Sakana Fugu API Key",
            required: false,
            description: "Sakana Fugu beta key",
        },
        SecretSchema {
            key: "sakana_fugu_api_url",
            category: "llm",
            label: "Sakana Fugu API URL",
            required: false,
            description: "Required beta OpenAI-compatible chat completions URL for provider=sakana_fugu",
        },
        SecretSchema {
            key: "openai_compatible_api_key",
            category: "llm",
            label: "OpenAI-Compatible API Key",
            required: false,
            description: "Bearer token for an arbitrary OpenAI-compatible endpoint",
        },
        SecretSchema {
            key: "openai_compatible_api_url",
            category: "llm",
            label: "OpenAI-Compatible API URL",
            required: false,
            description: "Required chat completions URL for provider=openai_compatible",
        },
        SecretSchema {
            key: "openai_compatible_headers_json",
            category: "llm",
            label: "OpenAI-Compatible Headers JSON",
            required: false,
            description: "Optional JSON object of extra string headers for custom OpenAI-compatible endpoints",
        },
        SecretSchema {
            key: "local_openai_api_url",
            category: "llm",
            label: "Local OpenAI API URL",
            required: false,
            description: "No-key local endpoint for provider=local_openai; defaults to Ollama at 127.0.0.1:11434",
        },
        SecretSchema {
            key: "llm_provider",
            category: "llm",
            label: "Active LLM Provider",
            required: false,
            description: "anthropic, openai, openai_codex, openrouter, huggingface, fireworks, sakana_fugu, local_openai, or openai_compatible",
        },
        SecretSchema {
            key: "llm_model",
            category: "llm",
            label: "LLM Model",
            required: false,
            description: "Configured model for the active LLM provider",
        },
        SecretSchema {
            key: "discord_bot_token",
            category: "messaging",
            label: "Discord Bot Token",
            required: false,
            description: "Bot token from Discord developer portal",
        },
        SecretSchema {
            key: "discord_public_key",
            category: "messaging",
            label: "Discord Public Key",
            required: false,
            description: "Optional override; auto-fetched from the bot token when possible",
        },
        SecretSchema {
            key: "discord_guild_id",
            category: "messaging",
            label: "Discord Guild ID",
            required: false,
            description: "Server ID for slash commands",
        },
        SecretSchema {
            key: "discord_feed_channel_id",
            category: "messaging",
            label: "Discord Feed Channel",
            required: false,
            description: "Channel for activity feed",
        },
        SecretSchema {
            key: "discord_forum_channel_id",
            category: "messaging",
            label: "Discord Forum Channel",
            required: false,
            description: "Forum channel for agent threads",
        },
        SecretSchema {
            key: "slack_app_token",
            category: "messaging",
            label: "Slack App Token",
            required: false,
            description: "xapp-... token for Socket Mode",
        },
        SecretSchema {
            key: "slack_bot_token",
            category: "messaging",
            label: "Slack Bot Token",
            required: false,
            description: "xoxb-... token for Web API",
        },
        SecretSchema {
            key: "slack_signing_secret",
            category: "messaging",
            label: "Slack Signing Secret",
            required: false,
            description: "Webhook signature verification",
        },
        SecretSchema {
            key: "exa_api_key",
            category: "web_search",
            label: "Exa API Key",
            required: false,
            description: "Web search via exa.ai — agents can research the internet",
        },
        SecretSchema {
            key: "sandbox_provider",
            category: "sandbox",
            label: "Sandbox Provider",
            required: false,
            description: "tensorlake or modal — where agents run code",
        },
        SecretSchema {
            key: "tensorlake_api_key",
            category: "sandbox",
            label: "TensorLake API Key",
            required: false,
            description: "Cloud sandbox provisioning — tensorlake.ai",
        },
        SecretSchema {
            key: "modal_token_id",
            category: "sandbox",
            label: "Modal Token ID",
            required: false,
            description: "Starts with ak-… — from modal.com/settings or `modal token set`",
        },
        SecretSchema {
            key: "modal_token_secret",
            category: "sandbox",
            label: "Modal Token Secret",
            required: false,
            description: "Starts with as-… — from modal.com/settings or `modal token set`",
        },
        SecretSchema {
            key: "github_token",
            category: "integrations",
            label: "GitHub Token",
            required: false,
            description: "For repo cloning and PR flows",
        },
        SecretSchema {
            key: "genesis_token",
            category: "integrations",
            label: "Genesis Token",
            required: false,
            description: "For pushing app bundles to the Genesis registry (Basic-auth username)",
        },
        // DD_* keys are infrastructure config set via Railway env vars (by `temperpaw deploy`).
        // They don't belong in the dashboard — change them in Railway if needed.
    ]
}

/// Build the `/paw/` router.
pub fn router(state: SetupApiState) -> Router {
    Router::new()
        .route("/discord/interaction", post(proxy_discord_interaction))
        .route("/paw/setup/status", get(get_setup_status))
        .route("/paw/setup/secrets", get(list_secrets))
        .route("/paw/setup/secrets/schema", get(get_secrets_schema))
        .route("/paw/setup/secrets", post(upsert_secret))
        .route("/paw/setup/secrets/{key}", get(get_secret))
        .route("/paw/setup/secrets/{key}", delete(delete_secret))
        .route(
            "/paw/setup/openai-codex/status",
            get(get_openai_codex_status),
        )
        .route(
            "/paw/setup/openai-codex/device-login",
            post(start_openai_codex_device_login),
        )
        .route(
            "/paw/apps/install-from-genesis",
            post(install_app_from_genesis),
        )
        .route(
            "/paw/setup/openai-codex/poll",
            post(poll_openai_codex_device_login),
        )
        .route(
            "/paw/setup/openai-codex/refresh",
            post(refresh_openai_codex_auth),
        )
        .route(
            "/paw/setup/openai-codex/ensure-fresh",
            post(ensure_fresh_openai_codex_auth),
        )
        .route(
            "/paw/setup/openai-codex/force-refresh",
            post(force_refresh_openai_codex_auth),
        )
        .route(
            "/paw/setup/openai-codex/disconnect",
            post(disconnect_openai_codex_auth),
        )
        .route("/paw/setup/soul", get(get_current_soul))
        .route("/paw/setup/soul/generate", post(generate_soul_preview))
        .route("/paw/setup/soul/save", post(save_soul))
        .route("/paw/souls/templates", get(list_soul_templates))
        .route("/paw/agents/create", post(create_agent))
        .route("/paw/transports/status", get(get_transport_status))
        .route("/paw/transports/discord/connect", post(connect_discord))
        .route(
            "/paw/internal/transports/discord/start",
            post(start_discord_internal),
        )
        .route(
            "/paw/transports/discord/disconnect",
            post(disconnect_discord),
        )
        .route("/paw/transports/slack/connect", post(connect_slack))
        .route("/paw/transports/slack/disconnect", post(disconnect_slack))
        .route("/paw/infra/railway/status", get(get_railway_status))
        .route("/paw/infra/railway/set-var", post(set_railway_var))
        .route(
            "/paw/infra/railway/datadog-runtime-agent/ensure",
            post(ensure_datadog_runtime_agent),
        )
        .route(
            "/paw/infra/railway/datadog-capability-check",
            get(get_datadog_railway_capability_check),
        )
        .route(
            "/paw/infra/railway/datadog-continuous-profiler-canary",
            post(set_datadog_continuous_profiler_canary),
        )
        .route(
            "/paw/infra/datadog/error-tracking-synthetic",
            post(emit_datadog_error_tracking_synthetic),
        )
        .route("/paw/infra/railway/redeploy", post(railway_redeploy))
        .route("/paw/version", get(get_version))
        .route("/paw/infra/updates", get(check_for_updates))
        .route("/paw/infra/edge", get(check_edge_build))
        .with_state(state)
}

// ──────────────────────────────── Setup Status ────────────────────────────────

#[derive(Serialize)]
struct SetupStatus {
    has_anthropic_key: bool,
    llm_provider: Option<String>,
    has_discord: bool,
    has_slack: bool,
    has_agents: bool,
    agent_count: usize,
    has_personalized_soul: bool,
    discord_connected: bool,
    slack_connected: bool,
    discord_interaction_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAICodexAuthStatus {
    configured: bool,
    status: Option<String>,
    verification_url: Option<String>,
    user_code: Option<String>,
    expires_at_ms: Option<String>,
    account_id: Option<String>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TransportStatusReport {
    status: String,
    configured: bool,
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    guild_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desired_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interaction_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct TransportStatusResponse {
    discord: TransportStatusReport,
    slack: TransportStatusReport,
}

#[derive(Debug, Clone)]
struct TransportConnectionSnapshot {
    status: String,
    fields: serde_json::Value,
    counters: std::collections::BTreeMap<String, usize>,
}

fn secret_is_configured(value: Option<String>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn field_str<'a>(fields: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(serde_json::Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

fn transport_status_report(
    configured: bool,
    runtime: &TransportStatus,
    desired_state: Option<&str>,
    connection_state: Option<&str>,
) -> TransportStatusReport {
    let (status, connected, guild_id, message) = match runtime {
        TransportStatus::Disconnected => ("disconnected".to_string(), false, None, None),
        TransportStatus::Connecting => ("connecting".to_string(), false, None, None),
        TransportStatus::Connected { guild_id } => {
            ("connected".to_string(), true, guild_id.clone(), None)
        }
        TransportStatus::Error { message } => {
            ("error".to_string(), false, None, Some(message.clone()))
        }
    };

    TransportStatusReport {
        status,
        configured,
        connected,
        guild_id,
        message,
        desired_state: desired_state.map(str::to_string),
        connection_state: connection_state.map(str::to_string),
        last_error: None,
        next_retry_at: None,
        interaction_url: None,
        attempt_count: None,
    }
}

fn transport_status_report_with_connection(
    configured: bool,
    runtime: &TransportStatus,
    connection: Option<&TransportConnectionSnapshot>,
) -> TransportStatusReport {
    let mut report = transport_status_report(
        configured,
        runtime,
        connection
            .and_then(|snapshot| field_str(&snapshot.fields, &["desired_state", "DesiredState"])),
        connection.map(|snapshot| snapshot.status.as_str()),
    );

    if let Some(snapshot) = connection {
        report.last_error = field_str(&snapshot.fields, &["last_error", "LastError"])
            .map(str::to_string)
            .or_else(|| {
                field_str(&snapshot.fields, &["error_message", "ErrorMessage"]).map(str::to_string)
            })
            .or_else(|| report.message.clone());
        report.next_retry_at =
            field_str(&snapshot.fields, &["next_retry_at", "NextRetryAt"]).map(str::to_string);
        report.interaction_url =
            field_str(&snapshot.fields, &["interaction_url", "InteractionUrl"]).map(str::to_string);
        report.attempt_count = snapshot.counters.get("attempt_count").copied();
    }

    report
}

fn openai_codex_configured(state: &SetupApiState) -> bool {
    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        return false;
    };
    [
        OPENAI_CODEX_ACCESS_TOKEN,
        OPENAI_CODEX_REFRESH_TOKEN,
        OPENAI_CODEX_EXPIRES_AT_MS,
        OPENAI_CODEX_ACCOUNT_ID,
    ]
    .iter()
    .all(|key| secret_is_configured(vault.get_secret(&state.tenant, key)))
}

async fn openai_codex_auth_snapshot(state: &SetupApiState) -> Option<TransportConnectionSnapshot> {
    let tenant_id = TenantId::new(&state.tenant);
    if !state.platform.server.entity_exists(
        &tenant_id,
        OPENAI_CODEX_AUTH_ENTITY_TYPE,
        OPENAI_CODEX_AUTH_ENTITY_ID,
    ) {
        return None;
    }

    state
        .platform
        .server
        .get_tenant_entity_state(
            &tenant_id,
            OPENAI_CODEX_AUTH_ENTITY_TYPE,
            OPENAI_CODEX_AUTH_ENTITY_ID,
        )
        .await
        .ok()
        .map(|response| TransportConnectionSnapshot {
            status: response.state.status,
            fields: response.state.fields,
            counters: response.state.counters,
        })
}

fn openai_codex_status_from_snapshot(
    configured: bool,
    snapshot: Option<TransportConnectionSnapshot>,
) -> OpenAICodexAuthStatus {
    let Some(snapshot) = snapshot else {
        return OpenAICodexAuthStatus {
            configured,
            status: None,
            verification_url: None,
            user_code: None,
            expires_at_ms: None,
            account_id: None,
            last_error: None,
        };
    };
    OpenAICodexAuthStatus {
        configured,
        status: Some(snapshot.status),
        verification_url: field_str(&snapshot.fields, &["verification_url", "VerificationUrl"])
            .map(str::to_string),
        user_code: field_str(&snapshot.fields, &["user_code", "UserCode"]).map(str::to_string),
        expires_at_ms: field_str(&snapshot.fields, &["expires_at_ms", "ExpiresAtMs"])
            .map(str::to_string),
        account_id: field_str(&snapshot.fields, &["account_id", "AccountId"]).map(str::to_string),
        last_error: field_str(
            &snapshot.fields,
            &["last_error", "LastError", "error_message", "ErrorMessage"],
        )
        .map(str::to_string),
    }
}

async fn ensure_openai_codex_auth_entity(state: &SetupApiState) -> Result<()> {
    let tenant_id = TenantId::new(&state.tenant);
    state
        .platform
        .server
        .get_or_create_tenant_entity(
            &tenant_id,
            OPENAI_CODEX_AUTH_ENTITY_TYPE,
            OPENAI_CODEX_AUTH_ENTITY_ID,
            serde_json::json!({ "id": OPENAI_CODEX_AUTH_ENTITY_ID }),
        )
        .await
        .map_err(|error| anyhow!("create OpenAICodexAuth failed: {error}"))?;
    Ok(())
}

async fn dispatch_openai_codex_auth_action(
    state: &SetupApiState,
    action: &str,
) -> Result<OpenAICodexAuthStatus> {
    ensure_openai_codex_auth_entity(state).await?;
    let tenant_id = TenantId::new(&state.tenant);
    let system = AgentContext::system();
    state
        .platform
        .server
        .dispatch_tenant_action_ext(
            &tenant_id,
            OPENAI_CODEX_AUTH_ENTITY_TYPE,
            OPENAI_CODEX_AUTH_ENTITY_ID,
            action,
            serde_json::json!({}),
            DispatchExtOptions {
                agent_ctx: &system,
                await_integration: true,
                await_reactions: true,
            },
        )
        .await
        .map_err(|error| anyhow!("OpenAICodexAuth.{action} failed: {error}"))?;

    let snapshot = openai_codex_auth_snapshot(state).await;
    Ok(openai_codex_status_from_snapshot(
        openai_codex_configured(state),
        snapshot,
    ))
}

fn discord_readyz_response(
    configured: bool,
    runtime: &TransportStatus,
    desired_state: Option<&str>,
    connection_state: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let report = transport_status_report(configured, runtime, desired_state, connection_state);
    let ready = !configured || report.connected;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = serde_json::json!({
        "status": if ready { "ready" } else { "degraded" },
        "healthz": "/healthz",
        "discord": report,
    });
    (status, body)
}

fn discord_start_error_is_retryable(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        " 429 ",
        "429 too many requests",
        " 500 ",
        "500 internal server error",
        " 502 ",
        "502 bad gateway",
        " 503 ",
        "503 service unavailable",
        " 504 ",
        "504 gateway timeout",
        "timed out",
        "timeout",
        "connection refused",
        "connection reset",
        "request sending failed",
        "error sending request",
        "startup task ended unexpectedly",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn discord_start_failure_status(error: &str) -> StatusCode {
    if discord_start_error_is_retryable(error) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::BAD_REQUEST
    }
}

fn discord_connect_params_from_vault(
    state: &SetupApiState,
) -> Result<Option<DiscordConnectParams>> {
    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        return Err(anyhow!("secrets vault is not configured"));
    };

    let Some(bot_token) = vault
        .get_secret(&state.tenant, "discord_bot_token")
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };

    Ok(Some(DiscordConnectParams {
        bot_token,
        public_key: vault
            .get_secret(&state.tenant, "discord_public_key")
            .filter(|value| !value.trim().is_empty()),
        guild_id: vault
            .get_secret(&state.tenant, "discord_guild_id")
            .filter(|value| !value.trim().is_empty()),
        feed_channel_id: vault
            .get_secret(&state.tenant, "discord_feed_channel_id")
            .filter(|value| !value.trim().is_empty()),
        forum_channel_id: vault
            .get_secret(&state.tenant, "discord_forum_channel_id")
            .filter(|value| !value.trim().is_empty()),
    }))
}

pub(crate) async fn schedule_discord_reconcile(
    platform: &PlatformState,
    tenant: &str,
) -> Result<()> {
    let tenant_id = TenantId::new(tenant);
    platform
        .server
        .get_or_create_tenant_entity(
            &tenant_id,
            "TransportConnection",
            DISCORD_TRANSPORT_CONNECTION_ID,
            serde_json::json!({
                "id": DISCORD_TRANSPORT_CONNECTION_ID,
                "platform": "discord",
                "desired_state": "connected",
            }),
        )
        .await
        .map_err(|error| anyhow!("create TransportConnection failed: {error}"))?;

    let system = AgentContext::system();
    platform
        .server
        .dispatch_tenant_action(
            &tenant_id,
            "TransportConnection",
            DISCORD_TRANSPORT_CONNECTION_ID,
            "Configure",
            serde_json::json!({
                "platform": "discord",
                "desired_state": "connected",
            }),
            &system,
        )
        .await
        .map_err(|error| anyhow!("configure Discord TransportConnection failed: {error}"))?;

    platform
        .server
        .dispatch_tenant_action(
            &tenant_id,
            "TransportConnection",
            DISCORD_TRANSPORT_CONNECTION_ID,
            "Start",
            serde_json::json!({}),
            &system,
        )
        .await
        .map_err(|error| anyhow!("start Discord TransportConnection failed: {error}"))?;

    Ok(())
}

async fn discord_transport_connection_snapshot(
    state: &SetupApiState,
) -> Option<TransportConnectionSnapshot> {
    let tenant_id = TenantId::new(&state.tenant);
    if !state.platform.server.entity_exists(
        &tenant_id,
        "TransportConnection",
        DISCORD_TRANSPORT_CONNECTION_ID,
    ) {
        return None;
    }

    state
        .platform
        .server
        .get_tenant_entity_state(
            &tenant_id,
            "TransportConnection",
            DISCORD_TRANSPORT_CONNECTION_ID,
        )
        .await
        .ok()
        .map(|response| TransportConnectionSnapshot {
            status: response.state.status,
            fields: response.state.fields,
            counters: response.state.counters,
        })
}

async fn get_setup_status(State(state): State<SetupApiState>) -> Json<SetupStatus> {
    let vault = state.platform.server.secrets_vault.as_ref();

    let has_anthropic_key = vault
        .and_then(|v| {
            v.get_secret(&state.tenant, "anthropic_api_key")
                .or_else(|| v.get_secret(&state.tenant, "openai_api_key"))
                .or_else(|| v.get_secret(&state.tenant, OPENAI_CODEX_ACCESS_TOKEN))
                .or_else(|| v.get_secret(&state.tenant, "openai_codex_token"))
                .or_else(|| v.get_secret(&state.tenant, "openrouter_api_key"))
                .or_else(|| v.get_secret(&state.tenant, "huggingface_api_key"))
                .or_else(|| v.get_secret(&state.tenant, "hf_token"))
                .or_else(|| v.get_secret(&state.tenant, "fireworks_api_key"))
                .or_else(|| v.get_secret(&state.tenant, "sakana_fugu_api_key"))
                .or_else(|| v.get_secret(&state.tenant, "openai_compatible_api_key"))
                .or_else(|| v.get_secret(&state.tenant, "openai_compatible_api_url"))
                .or_else(|| v.get_secret(&state.tenant, "local_openai_api_url"))
        })
        .is_some();
    let llm_provider = vault.and_then(|v| v.get_secret(&state.tenant, "llm_provider"));
    let has_discord =
        secret_is_configured(vault.and_then(|v| v.get_secret(&state.tenant, "discord_bot_token")));
    let has_slack =
        secret_is_configured(vault.and_then(|v| v.get_secret(&state.tenant, "slack_bot_token")));

    // Count agents from entity index
    let agent_count = {
        let index = state.platform.server.entity_index.read().unwrap();
        let key = format!("{}:Agent", state.tenant);
        index.get(&key).map(|set| set.len()).unwrap_or(0)
    };

    let transport_status = state.transport_manager.status().await;
    let discord_connected = matches!(
        transport_status.discord,
        crate::transport_manager::TransportStatus::Connected { .. }
    );
    let slack_connected = matches!(
        transport_status.slack,
        crate::transport_manager::TransportStatus::Connected { .. }
    );
    let has_personalized_soul = has_personalized_paw_soul(&state).await;

    Json(SetupStatus {
        has_anthropic_key,
        llm_provider,
        has_discord,
        has_slack,
        has_agents: agent_count > 0,
        agent_count,
        has_personalized_soul,
        discord_connected,
        slack_connected,
        discord_interaction_url: state
            .transport_manager
            .discord_interaction_public_url()
            .await,
    })
}

fn personalized_soul_flag_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

fn persisted_personalized_soul_flag(state: &SetupApiState) -> bool {
    personalized_soul_flag_value(
        state
            .platform
            .server
            .secrets_vault
            .as_ref()
            .and_then(|vault| vault.get_secret(&state.tenant, "paw_personalized_soul"))
            .as_deref(),
    )
}

async fn has_personalized_paw_soul(state: &SetupApiState) -> bool {
    if persisted_personalized_soul_flag(state) || has_local_personalized_paw_soul() {
        return true;
    }

    let Ok(default_content) = default_paw_soul_content() else {
        return false;
    };
    let Ok((_, current_content)) = load_current_paw_soul(state, &SetupRequestAuth::default()).await
    else {
        return false;
    };

    current_content.trim() != default_content.trim()
}

// ──────────────────────────────── Secrets ────────────────────────────────────

#[derive(Serialize)]
struct SecretKeyList {
    keys: Vec<String>,
}

#[derive(Serialize)]
struct SecretValueResponse {
    key: String,
    value: String,
}

async fn list_secrets(State(state): State<SetupApiState>) -> Json<SecretKeyList> {
    let keys = state
        .platform
        .server
        .secrets_vault
        .as_ref()
        .map(|vault| vault.list_keys(&state.tenant))
        .unwrap_or_default();
    Json(SecretKeyList { keys })
}

async fn get_secrets_schema() -> Json<Vec<SecretSchema>> {
    Json(secrets_schema())
}

async fn get_secret(
    State(state): State<SetupApiState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_setup_secret_key(&key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error })),
        );
    }

    if key == "temper_api_key" {
        return match state.platform.api_token.clone() {
            Some(value) => (
                StatusCode::OK,
                Json(serde_json::to_value(SecretValueResponse { key, value }).unwrap()),
            ),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Secret not found" })),
            ),
        };
    }

    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Vault not initialized" })),
        );
    };

    match vault.get_secret(&state.tenant, &key) {
        Some(value) => (
            StatusCode::OK,
            Json(serde_json::to_value(SecretValueResponse { key, value }).unwrap()),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Secret not found" })),
        ),
    }
}

async fn get_openai_codex_status(
    State(state): State<SetupApiState>,
) -> Json<OpenAICodexAuthStatus> {
    let snapshot = openai_codex_auth_snapshot(&state).await;
    Json(openai_codex_status_from_snapshot(
        openai_codex_configured(&state),
        snapshot,
    ))
}

#[derive(Debug, Deserialize)]
struct InstallFromGenesisRequest {
    /// Pinned ref `owner/name@hash`, e.g. `katagami/katagami-curation@<hash>`.
    app_ref: String,
    #[serde(default)]
    registry_url: String,
    #[serde(default)]
    registry_tenant: String,
    #[serde(default)]
    follow_policy: String,
}

fn setup_genesis_registry_url(raw: &str) -> Result<String, String> {
    let fallback = env::var("TEMPERPAW_GENESIS_REGISTRY_URL")
        .or_else(|_| env::var("TEMPER_GENESIS_REGISTRY_URL"))
        .unwrap_or_else(|_| DEFAULT_GENESIS_REGISTRY_URL.to_string());
    let value = if raw.trim().is_empty() {
        fallback.as_str()
    } else {
        raw
    }
    .trim()
    .trim_end_matches('/');

    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err("registry_url must start with http:// or https://".to_string());
    }
    Ok(value.to_string())
}

fn setup_genesis_registry_tenant(raw: &str) -> String {
    if raw.trim().is_empty() {
        "default".to_string()
    } else {
        raw.trim().to_string()
    }
}

fn setup_genesis_follow_policy(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized == "pinned" {
        return Ok("pinned".to_string());
    }
    if normalized == "follow_latest" || normalized == "follow-latest" {
        return Ok("follow_latest".to_string());
    }
    Err(format!(
        "follow_policy must be 'pinned' or 'follow_latest', got '{raw}'"
    ))
}

fn pinned_genesis_app_ref(raw: &str) -> Option<String> {
    let app_ref = raw.trim();
    let (owner_and_name, hash) = app_ref.split_once('@')?;
    if hash.trim().is_empty() || hash.contains('@') {
        return None;
    }
    let (owner, name) = owner_and_name.split_once('/')?;
    if owner.trim().is_empty() || name.trim().is_empty() || name.contains('/') {
        return None;
    }
    Some(format!(
        "{}/{}@{}",
        owner.trim(),
        name.trim(),
        hash.trim().trim_start_matches('@')
    ))
}

fn genesis_install_request_from_setup(
    tenant: &str,
    req: InstallFromGenesisRequest,
) -> Result<GenesisRegistryInstallRequest, String> {
    let app_ref = pinned_genesis_app_ref(&req.app_ref)
        .ok_or_else(|| "app_ref must be a pinned Genesis ref owner/name@hash".to_string())?;
    Ok(GenesisRegistryInstallRequest {
        tenant: tenant.to_string(),
        app_ref,
        registry_url: setup_genesis_registry_url(&req.registry_url)?,
        registry_tenant: setup_genesis_registry_tenant(&req.registry_tenant),
        follow_policy: setup_genesis_follow_policy(&req.follow_policy)?,
    })
}

/// `POST /paw/apps/install-from-genesis` — reconcile an OS app from a pinned Genesis
/// ref on a running pod, without a Docker rebuild. Privileged setup endpoint on the
/// same `/paw/` admin surface as secret/agent management. Lets katagami (and any
/// os-app) be updated by publishing to Genesis + hitting this endpoint, instead of a
/// full image rebuild (ARN-69).
async fn install_app_from_genesis(
    State(state): State<SetupApiState>,
    Json(req): Json<InstallFromGenesisRequest>,
) -> impl IntoResponse {
    let request = match genesis_install_request_from_setup(&state.tenant, req) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": error })),
            );
        }
    };
    match temper_platform::genesis_install::install_genesis_app_from_registry(
        &state.platform,
        request,
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        ),
    }
}

async fn start_openai_codex_device_login(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "StartDeviceLogin").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

async fn poll_openai_codex_device_login(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "PollDeviceLogin").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

async fn refresh_openai_codex_auth(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "Refresh").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

async fn ensure_fresh_openai_codex_auth(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "EnsureFresh").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

async fn force_refresh_openai_codex_auth(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "ForceRefresh").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

async fn disconnect_openai_codex_auth(State(state): State<SetupApiState>) -> impl IntoResponse {
    match dispatch_openai_codex_auth_action(&state, "Disconnect").await {
        Ok(status) => (StatusCode::OK, Json(serde_json::json!(status))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
struct UpsertSecretRequest {
    key: String,
    value: String,
}

fn discord_connect_params_for_secret_update<F>(
    get_secret: F,
    updated_key: &str,
    updated_value: &str,
) -> Option<DiscordConnectParams>
where
    F: Fn(&str) -> Option<String>,
{
    fn effective_secret<F>(
        get_secret: &F,
        updated_key: &str,
        updated_value: &str,
        key: &str,
    ) -> Option<String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let value = if key == updated_key {
            Some(updated_value.to_string())
        } else {
            get_secret(key)
        }?;

        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    if !matches!(
        updated_key,
        "discord_bot_token"
            | "discord_public_key"
            | "discord_guild_id"
            | "discord_feed_channel_id"
            | "discord_forum_channel_id"
    ) {
        return None;
    }

    Some(DiscordConnectParams {
        bot_token: effective_secret(&get_secret, updated_key, updated_value, "discord_bot_token")?,
        public_key: effective_secret(
            &get_secret,
            updated_key,
            updated_value,
            "discord_public_key",
        ),
        guild_id: effective_secret(&get_secret, updated_key, updated_value, "discord_guild_id"),
        feed_channel_id: effective_secret(
            &get_secret,
            updated_key,
            updated_value,
            "discord_feed_channel_id",
        ),
        forum_channel_id: effective_secret(
            &get_secret,
            updated_key,
            updated_value,
            "discord_forum_channel_id",
        ),
    })
}

async fn upsert_secret(
    State(state): State<SetupApiState>,
    Json(req): Json<UpsertSecretRequest>,
) -> impl IntoResponse {
    if let Err(error) = validate_setup_secret_key(&req.key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error })),
        );
    }

    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Vault not initialized" })),
        );
    };

    let should_schedule_discord = discord_connect_params_for_secret_update(
        |key| {
            vault
                .get_secret(&state.tenant, key)
                .or_else(|| vault.get_secret("default", key))
        },
        &req.key,
        &req.value,
    );

    // Cache in memory + persist to Turso
    let _ = vault.cache_secret(&state.tenant, &req.key, req.value.clone());

    if let Ok((ciphertext, nonce)) = vault.encrypt(req.value.as_bytes()) {
        let _ = state
            .storage
            .upsert_secret(&state.tenant, &req.key, &ciphertext, &nonce)
            .await;
    }

    let mut response = serde_json::json!({ "saved": req.key });
    if should_schedule_discord.is_some() {
        match schedule_discord_reconcile(&state.platform, &state.tenant).await {
            Ok(()) => response["discord_reconcile"] = serde_json::json!("scheduled"),
            Err(error) => {
                tracing::warn!(%error, "Saved Discord secret but could not schedule reconcile");
                response["discord_reconcile"] = serde_json::json!("schedule_failed");
                response["discord_reconcile_error"] = serde_json::json!(error.to_string());
            }
        }
    }

    (StatusCode::OK, Json(response))
}

async fn delete_secret(
    State(state): State<SetupApiState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_setup_secret_key(&key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error })),
        );
    }

    if let Some(vault) = state.platform.server.secrets_vault.as_ref() {
        vault.remove_secret(&state.tenant, &key);
    }
    let _ = state.storage.delete_secret(&state.tenant, &key).await;

    (StatusCode::OK, Json(serde_json::json!({ "deleted": key })))
}

// ───────────────────────────── Soul Personalization ───────────────────────────

#[derive(Serialize)]
struct CurrentSoulResponse {
    summary: String,
    content: String,
}

#[derive(Deserialize)]
struct GenerateSoulRequest {
    interview: UserInterview,
    previous_summary: Option<String>,
    feedback: Option<String>,
}

async fn get_current_soul(
    State(state): State<SetupApiState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match load_current_paw_soul(&state, &SetupRequestAuth::from_headers(&headers)).await {
        Ok((summary, content)) => (
            StatusCode::OK,
            Json(serde_json::to_value(CurrentSoulResponse { summary, content }).unwrap()),
        ),
        Err(error) => {
            tracing::warn!(%error, "Could not load current soul");
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Paw soul not found" })),
            )
        }
    }
}

async fn generate_soul_preview(
    State(state): State<SetupApiState>,
    Json(request): Json<GenerateSoulRequest>,
) -> impl IntoResponse {
    match resolve_llm_provider(&state).await {
        Ok(provider) => {
            let generated = if let Some(feedback) = request.feedback.as_deref() {
                refine_soul(
                    &provider,
                    &request.interview,
                    request.previous_summary.as_deref().unwrap_or_default(),
                    feedback,
                )
                .await
            } else {
                generate_personalized_soul(&provider, &request.interview).await
            };

            match generated {
                Ok(soul) => {
                    (StatusCode::OK, Json(serde_json::to_value(soul).unwrap())).into_response()
                }
                Err(error) => {
                    tracing::warn!(%error, "Soul generation failed");
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": error.to_string() })),
                    )
                        .into_response()
                }
            }
        }
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn save_soul(
    State(state): State<SetupApiState>,
    headers: HeaderMap,
    Json(generated): Json<GeneratedSoul>,
) -> impl IntoResponse {
    let client = reqwest::Client::new();
    let auth = SetupRequestAuth::from_headers(&headers);
    match save_soul_to_temper(&client, &state.base_url, &state.tenant, &generated, &auth).await {
        Ok(()) => {
            if let Some(vault) = state.platform.server.secrets_vault.as_ref() {
                let _ =
                    vault.cache_secret(&state.tenant, "paw_personalized_soul", "true".to_string());
                if let Ok((ciphertext, nonce)) = vault.encrypt(b"true") {
                    let _ = state
                        .storage
                        .upsert_secret(&state.tenant, "paw_personalized_soul", &ciphertext, &nonce)
                        .await;
                }
            }

            (StatusCode::OK, Json(serde_json::json!({ "saved": true }))).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "Saving personalized soul failed");
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response()
        }
    }
}

async fn resolve_llm_provider(state: &SetupApiState) -> Result<LlmProvider> {
    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        anyhow::bail!("Vault not initialized");
    };

    let provider_hint = vault
        .get_secret(&state.tenant, "llm_provider")
        .context("Configure llm_provider before personalizing Paw")?;
    let model = vault
        .get_secret(&state.tenant, "llm_model")
        .context("Configure llm_model before personalizing Paw")?;
    let provider = normalize_setup_provider(&provider_hint);

    match provider.as_str() {
        "local_openai" => {
            let api_url = vault
                .get_secret(&state.tenant, "local_openai_api_url")
                .unwrap_or_else(|| "http://127.0.0.1:11434/v1/chat/completions".to_string());
            LlmProvider::openai_compatible(&provider, "", &api_url, &model)
        }
        "huggingface" => {
            let api_key = vault
                .get_secret(&state.tenant, "huggingface_api_key")
                .or_else(|| vault.get_secret(&state.tenant, "hf_token"))
                .context("Configure huggingface_api_key before personalizing Paw")?;
            let api_url = vault
                .get_secret(&state.tenant, "huggingface_api_url")
                .unwrap_or_else(|| "https://router.huggingface.co/v1/chat/completions".to_string());
            LlmProvider::openai_compatible(&provider, &api_key, &api_url, &model)
        }
        "fireworks" => {
            let api_key = vault
                .get_secret(&state.tenant, "fireworks_api_key")
                .context("Configure fireworks_api_key before personalizing Paw")?;
            let api_url = vault
                .get_secret(&state.tenant, "fireworks_api_url")
                .unwrap_or_else(|| {
                    "https://api.fireworks.ai/inference/v1/chat/completions".to_string()
                });
            LlmProvider::openai_compatible(&provider, &api_key, &api_url, &model)
        }
        "sakana_fugu" => {
            let api_key = vault
                .get_secret(&state.tenant, "sakana_fugu_api_key")
                .context("Configure sakana_fugu_api_key before personalizing Paw")?;
            let api_url = vault
                .get_secret(&state.tenant, "sakana_fugu_api_url")
                .context("Configure sakana_fugu_api_url before personalizing Paw")?;
            LlmProvider::openai_compatible(&provider, &api_key, &api_url, &model)
        }
        "openai_compatible" => {
            let api_key = vault
                .get_secret(&state.tenant, "openai_compatible_api_key")
                .unwrap_or_default();
            let api_url = vault
                .get_secret(&state.tenant, "openai_compatible_api_url")
                .context("Configure openai_compatible_api_url before personalizing Paw")?;
            LlmProvider::openai_compatible(&provider, &api_key, &api_url, &model)
        }
        _ => {
            let api_key = match provider.as_str() {
                "anthropic" => vault.get_secret(&state.tenant, "anthropic_api_key"),
                "openrouter" => vault.get_secret(&state.tenant, "openrouter_api_key"),
                "openai" => vault.get_secret(&state.tenant, "openai_api_key"),
                "openai_codex" => vault
                    .get_secret(&state.tenant, OPENAI_CODEX_ACCESS_TOKEN)
                    .or_else(|| vault.get_secret(&state.tenant, "openai_codex_token")),
                _ => [
                    "anthropic_api_key",
                    "openrouter_api_key",
                    "openai_api_key",
                    OPENAI_CODEX_ACCESS_TOKEN,
                    "openai_codex_token",
                ]
                .into_iter()
                .find_map(|key| vault.get_secret(&state.tenant, key)),
            }
            .with_context(|| format!("Configure an LLM API key for provider {provider}"))?;
            LlmProvider::detect(&api_key, &provider, &model)
        }
    }
}

fn normalize_setup_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "open_router" => "openrouter".to_string(),
        "hf" | "hugging_face" | "hugging-face" => "huggingface".to_string(),
        "fireworks_ai" | "fireworks-ai" => "fireworks".to_string(),
        "sakana" | "sakana-fugu" | "fugu" => "sakana_fugu".to_string(),
        "ollama" | "local" | "local-openai" => "local_openai".to_string(),
        "openai-compatible" | "openai_compat" | "openai-compat" | "custom_openai" => {
            "openai_compatible".to_string()
        }
        other => other.to_string(),
    }
}

async fn load_current_paw_soul(
    state: &SetupApiState,
    auth: &SetupRequestAuth,
) -> Result<(String, String)> {
    let client = reqwest::Client::new();
    load_paw_soul_content(&client, &state.base_url, &state.tenant, auth)
        .await
        .context("Failed to load Paw soul content")
}

// ──────────────────────────────── Soul Templates ─────────────────────────────

#[derive(Serialize)]
struct SoulTemplate {
    name: String,
    description: String,
    path: String,
}

#[derive(Serialize)]
struct SoulTemplateList {
    templates: Vec<SoulTemplate>,
}

async fn list_soul_templates(State(state): State<SetupApiState>) -> Json<SoulTemplateList> {
    let mut templates = Vec::new();

    // Scan os-apps/paw-agent/agents/ for agent templates (new app structure).
    // Each subdirectory is an agent with AGENT.md (operations manual) and optionally SOUL.md.
    if let Ok(entries) = std::fs::read_dir(&state.agents_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();

            // Prefer AGENT.md for description, fall back to SOUL.md
            let desc_file = if path.join("AGENT.md").exists() {
                path.join("AGENT.md")
            } else if path.join("SOUL.md").exists() {
                path.join("SOUL.md")
            } else {
                continue;
            };

            let desc = read_first_line(&desc_file).unwrap_or_default();
            templates.push(SoulTemplate {
                name,
                description: desc,
                path: desc_file.to_string_lossy().to_string(),
            });
        }
    }

    templates.sort_by(|a, b| a.name.cmp(&b.name));
    Json(SoulTemplateList { templates })
}

fn read_first_line(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content
        .lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|s| s.trim().to_string())
}

// ──────────────────────────────── Agent Creation ─────────────────────────────

#[derive(Deserialize)]
struct CreateAgentRequest {
    name: String,
    role: Option<String>,
    _soul_template: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    tools_enabled: Option<String>,
    max_turns: Option<String>,
}

async fn create_agent(
    State(state): State<SetupApiState>,
    Json(req): Json<CreateAgentRequest>,
) -> impl IntoResponse {
    let tenant_id = temper_runtime::tenant::TenantId::new(&state.tenant);
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4());

    // Create Agent entity
    let fields = serde_json::json!({
        "name": req.name,
        "role": req.role.unwrap_or_default(),
    });

    match state
        .platform
        .server
        .get_or_create_tenant_entity(&tenant_id, "Agent", &agent_id, fields)
        .await
    {
        Ok(_) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to create agent: {e}") })),
            );
        }
    }

    // Dispatch Configure action — resolve provider/model from explicit request
    // or tenant-level Temper secrets. Do not infer runtime LLM config.
    let vault = state.platform.server.secrets_vault.as_ref();
    let resolved_provider = req
        .provider
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            vault
                .and_then(|v| v.get_secret(&state.tenant, "llm_provider"))
                .filter(|value| !value.trim().is_empty())
        });
    let resolved_model = req
        .model
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            vault
                .and_then(|v| v.get_secret(&state.tenant, "llm_model"))
                .filter(|value| !value.trim().is_empty())
        });
    let Some(resolved_provider) = resolved_provider else {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": "Agent creation requires provider or tenant llm_provider" }),
            ),
        );
    };
    let Some(resolved_model) = resolved_model else {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": "Agent creation requires model or tenant llm_model" }),
            ),
        );
    };
    let configure_params = serde_json::json!({
        "model": resolved_model,
        "provider": resolved_provider,
        "tools_enabled": req.tools_enabled.unwrap_or_else(|| DEFAULT_SETUP_AGENT_TOOLS_ENABLED.to_string()),
        "workdir": "/workspace",
        "max_turns": req.max_turns.unwrap_or_else(|| "20".to_string()),
    });

    match state
        .platform
        .server
        .dispatch_tenant_action(
            &tenant_id,
            "Agent",
            &agent_id,
            "Configure",
            configure_params,
            &temper_server::request_context::AgentContext::system(),
        )
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "agent_id": agent_id,
                "name": req.name,
                "status": "Active"
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to configure agent: {e}") })),
        ),
    }
}

// ──────────────────────────── Railway Integration ────────────────────────────

#[derive(Serialize)]
struct RailwayStatus {
    configured: bool,
    can_update: bool,
    project_id: Option<String>,
    environment_id: Option<String>,
    service_id: Option<String>,
    otel_service_id: Option<String>,
    datadog_runtime_agent_service_id: Option<String>,
}

async fn get_railway_status(State(state): State<SetupApiState>) -> Json<RailwayStatus> {
    let vault = state.platform.server.secrets_vault.as_ref();
    let has_token = vault
        .and_then(|v| v.get_secret(&state.tenant, "railway_token"))
        .is_some();
    let project_id = vault.and_then(|v| v.get_secret(&state.tenant, "railway_project_id"));
    let environment_id = vault.and_then(|v| v.get_secret(&state.tenant, "railway_environment_id"));
    let service_id = vault.and_then(|v| v.get_secret(&state.tenant, "railway_service_id"));
    let otel_service_id =
        vault.and_then(|v| v.get_secret(&state.tenant, "railway_otel_service_id"));
    let datadog_runtime_agent_service_id =
        vault.and_then(|v| v.get_secret(&state.tenant, "railway_datadog_runtime_agent_service_id"));
    let configured = has_token && project_id.is_some() && environment_id.is_some();
    let can_update = configured && service_id.is_some();
    Json(RailwayStatus {
        configured,
        can_update,
        project_id,
        environment_id,
        service_id,
        otel_service_id,
        datadog_runtime_agent_service_id,
    })
}

#[derive(Deserialize)]
struct SetRailwayVarRequest {
    service: String,
    key: String,
    value: String,
}

/// Allowlist of (service, key) pairs that can be set via this endpoint.
fn allowed_railway_vars() -> Vec<(&'static str, &'static str)> {
    vec![
        ("otel-collector", "DD_API_KEY"),
        ("otel-collector", "DD_SITE"),
        ("datadog-runtime-agent", "DD_API_KEY"),
        ("datadog-runtime-agent", "DD_SITE"),
    ]
}

async fn set_railway_var(
    State(state): State<SetupApiState>,
    Json(req): Json<SetRailwayVarRequest>,
) -> impl IntoResponse {
    // Validate against allowlist
    if !allowed_railway_vars()
        .iter()
        .any(|(s, k)| *s == req.service && *k == req.key)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Setting {} on {} is not allowed", req.key, req.service)
            })),
        )
            .into_response();
    }

    let vault = match state.platform.server.secrets_vault.as_ref() {
        Some(v) => v,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Vault not initialized" })),
            )
                .into_response();
        }
    };

    let railway_token = vault.get_secret(&state.tenant, "railway_token");
    let project_id = vault.get_secret(&state.tenant, "railway_project_id");
    let environment_id = vault.get_secret(&state.tenant, "railway_environment_id");

    let (Some(token), Some(project), Some(env)) = (railway_token, project_id, environment_id)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Railway integration not configured. Deploy with `temperpaw deploy` first."
            })),
        )
            .into_response();
    };

    // Resolve service ID — for otel-collector, read from vault
    let service_id = match req.service.as_str() {
        "otel-collector" => vault.get_secret(&state.tenant, "railway_otel_service_id"),
        "datadog-runtime-agent" => {
            vault.get_secret(&state.tenant, "railway_datadog_runtime_agent_service_id")
        }
        _ => None,
    };

    let Some(service_id) = service_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("Service ID for {} not found in vault", req.service)
            })),
        )
            .into_response();
    };

    // Call Railway GraphQL API — variableUpsert
    let graphql_query = serde_json::json!({
        "query": "mutation($input: VariableUpsertInput!) { variableUpsert(input: $input) }",
        "variables": {
            "input": {
                "projectId": project,
                "environmentId": env,
                "serviceId": service_id,
                "name": req.key,
                "value": req.value,
            }
        }
    });

    let client = reqwest::Client::new();
    match client
        .post("https://backboard.railway.com/graphql/v2")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&graphql_query)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            if status.is_success() && body.get("errors").is_none() {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "set": req.key,
                        "service": req.service,
                    })),
                )
                    .into_response()
            } else {
                let error_msg = body["errors"]
                    .as_array()
                    .and_then(|e| e.first())
                    .and_then(|e| e["message"].as_str())
                    .unwrap_or("Railway API error");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": error_msg })),
                )
                    .into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Railway API request failed: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct EnsureDatadogRuntimeAgentResponse {
    ensured: bool,
    service_id: String,
    service_name: &'static str,
    created: bool,
    app_variables_set: usize,
    runtime_agent_variables_set: usize,
    runtime_agent_redeploy_triggered: bool,
    app_redeploy_triggered: bool,
    datadog_profile: &'static str,
}

async fn ensure_datadog_runtime_agent(State(state): State<SetupApiState>) -> impl IntoResponse {
    let vault = match state.platform.server.secrets_vault.as_ref() {
        Some(v) => v,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Vault not initialized" })),
            )
                .into_response();
        }
    };

    let railway_token = vault.get_secret(&state.tenant, "railway_token");
    let project_id = vault.get_secret(&state.tenant, "railway_project_id");
    let environment_id = vault.get_secret(&state.tenant, "railway_environment_id");
    let app_service_id = vault.get_secret(&state.tenant, "railway_service_id");
    let dd_api_key = vault
        .get_secret(&state.tenant, "dd_api_key")
        .or_else(|| vault.get_secret("default", "dd_api_key"));
    let dd_site = vault
        .get_secret(&state.tenant, "dd_site")
        .or_else(|| vault.get_secret("default", "dd_site"))
        .unwrap_or_else(|| "datadoghq.com".to_string());

    let (Some(token), Some(project), Some(env), Some(app_svc), Some(dd_key)) = (
        railway_token,
        project_id,
        environment_id,
        app_service_id,
        dd_api_key,
    ) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Railway Datadog Runtime Agent ensure requires railway_token, railway_project_id, railway_environment_id, railway_service_id, and dd_api_key."
            })),
        )
            .into_response();
    };

    let client = reqwest::Client::new();
    let result = async {
        let existing_service = railway_find_service_by_name(
            &client,
            &token,
            &project,
            DATADOG_RUNTIME_AGENT_SERVICE_NAME,
        )
        .await?;
        let (runtime_service_id, created) = match existing_service {
            Some(service_id) => (service_id, false),
            None => {
                let service_id = railway_create_datadog_runtime_agent_service(
                    &client, &token, &project, &env, &dd_key, &dd_site,
                )
                .await?;
                (service_id, true)
            }
        };

        railway_update_service_source_image(
            &client,
            &token,
            &env,
            &runtime_service_id,
            DATADOG_RUNTIME_AGENT_IMAGE,
        )
        .await?;

        let runtime_agent_vars = datadog_runtime_agent_railway_vars(&dd_key, &dd_site);
        for (name, value) in &runtime_agent_vars {
            railway_upsert_variable(
                &client,
                &token,
                &project,
                &env,
                &runtime_service_id,
                name,
                value,
            )
            .await?;
        }

        let app_vars = datadog_enhanced_app_railway_vars(&dd_key, &dd_site, &state.build_sha);
        for (name, value) in &app_vars {
            railway_upsert_variable(&client, &token, &project, &env, &app_svc, name, value).await?;
        }

        persist_infra_secret(
            vault,
            &state.storage,
            &state.tenant,
            "railway_datadog_runtime_agent_service_id",
            &runtime_service_id,
        )
        .await;

        railway_redeploy_service(&client, &token, &env, &runtime_service_id).await?;
        railway_redeploy_service(&client, &token, &env, &app_svc).await?;

        Ok::<EnsureDatadogRuntimeAgentResponse, anyhow::Error>(EnsureDatadogRuntimeAgentResponse {
            ensured: true,
            service_id: runtime_service_id,
            service_name: DATADOG_RUNTIME_AGENT_SERVICE_NAME,
            created,
            app_variables_set: app_vars.len(),
            runtime_agent_variables_set: runtime_agent_vars.len(),
            runtime_agent_redeploy_triggered: true,
            app_redeploy_triggered: true,
            datadog_profile: "datadog-enhanced-railway",
        })
    }
    .await;

    match result {
        Ok(response) => (StatusCode::OK, Json(serde_json::json!(response))).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct DatadogRailwayCapabilityReport {
    usm_status: &'static str,
    continuous_profiler_status: &'static str,
    system_probe: DatadogSystemProbeCapabilityReport,
    continuous_profiler: DatadogContinuousProfilerCapabilityReport,
}

#[derive(Serialize)]
struct DatadogSystemProbeCapabilityReport {
    #[serde(rename = "DD_SYSTEM_PROBE_SERVICE_MONITORING_ENABLED")]
    dd_system_probe_service_monitoring_enabled: String,
    #[serde(rename = "CAP_SYS_ADMIN")]
    cap_sys_admin: bool,
    #[serde(rename = "CAP_SYS_RESOURCE")]
    cap_sys_resource: bool,
    #[serde(rename = "CAP_SYS_PTRACE")]
    cap_sys_ptrace: bool,
    #[serde(rename = "CAP_NET_ADMIN")]
    cap_net_admin: bool,
    #[serde(rename = "CAP_NET_RAW")]
    cap_net_raw: bool,
    #[serde(rename = "CAP_IPC_LOCK")]
    cap_ipc_lock: bool,
    #[serde(rename = "CAP_CHOWN")]
    cap_chown: bool,
    host_proc: bool,
    host_cgroup: bool,
    debugfs: bool,
    lib_modules: bool,
}

#[derive(Serialize)]
struct DatadogContinuousProfilerCapabilityReport {
    #[serde(rename = "TEMPER_DDPROF_ENABLED")]
    temper_ddprof_enabled: String,
    ddprof_present: bool,
    perf_event_paranoid: String,
    #[serde(rename = "CAP_PERFMON")]
    cap_perfmon: bool,
}

async fn get_datadog_railway_capability_check() -> Json<DatadogRailwayCapabilityReport> {
    Json(datadog_railway_capability_report())
}

fn datadog_railway_capability_report() -> DatadogRailwayCapabilityReport {
    let system_probe_enabled = env::var("DD_SYSTEM_PROBE_SERVICE_MONITORING_ENABLED")
        .unwrap_or_else(|_| "false".to_string());
    let cap_sys_admin = effective_capability_bit(21);
    let cap_sys_resource = effective_capability_bit(24);
    let cap_sys_ptrace = effective_capability_bit(19);
    let cap_net_admin = effective_capability_bit(12);
    let cap_net_raw = effective_capability_bit(13);
    let cap_ipc_lock = effective_capability_bit(14);
    let cap_chown = effective_capability_bit(0);
    let cap_perfmon = effective_capability_bit(38);
    let host_proc = std::path::Path::new("/host/proc").exists();
    let host_cgroup = std::path::Path::new("/host/sys/fs/cgroup").exists();
    let debugfs = std::path::Path::new("/sys/kernel/debug").exists();
    let lib_modules = std::path::Path::new("/lib/modules").exists();

    let system_probe_host_ready = cap_sys_admin
        && cap_sys_resource
        && cap_sys_ptrace
        && cap_net_admin
        && cap_net_raw
        && cap_ipc_lock
        && cap_chown
        && host_proc
        && host_cgroup
        && debugfs
        && lib_modules;
    let usm_status = if system_probe_host_ready && system_probe_enabled == "true" {
        "supported"
    } else if system_probe_host_ready {
        "best-effort-system-probe-not-enabled"
    } else {
        "blocked-on-Railway-system-probe"
    };

    let temper_ddprof_enabled =
        env::var("TEMPER_DDPROF_ENABLED").unwrap_or_else(|_| "false".to_string());
    let ddprof_present = command_exists_on_path("ddprof");
    let perf_event_paranoid = fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let continuous_profiler_status = if temper_ddprof_enabled == "true" {
        if ddprof_present && perf_allows_unprivileged_profiling(&perf_event_paranoid, cap_perfmon) {
            "supported"
        } else {
            "blocked-on-Railway-perf-permissions"
        }
    } else {
        "best-effort-canary-not-enabled"
    };

    DatadogRailwayCapabilityReport {
        usm_status,
        continuous_profiler_status,
        system_probe: DatadogSystemProbeCapabilityReport {
            dd_system_probe_service_monitoring_enabled: system_probe_enabled,
            cap_sys_admin,
            cap_sys_resource,
            cap_sys_ptrace,
            cap_net_admin,
            cap_net_raw,
            cap_ipc_lock,
            cap_chown,
            host_proc,
            host_cgroup,
            debugfs,
            lib_modules,
        },
        continuous_profiler: DatadogContinuousProfilerCapabilityReport {
            temper_ddprof_enabled,
            ddprof_present,
            perf_event_paranoid,
            cap_perfmon,
        },
    }
}

fn effective_capability_bit(bit: u32) -> bool {
    let Some(cap_eff) = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("CapEff:")
                    .map(str::trim)
                    .and_then(|hex| u128::from_str_radix(hex, 16).ok())
            })
        })
    else {
        return false;
    };

    (cap_eff & (1u128 << bit)) != 0
}

fn command_exists_on_path(command: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| {
            env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(command);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn perf_allows_unprivileged_profiling(perf_event_paranoid: &str, cap_perfmon: bool) -> bool {
    match perf_event_paranoid.parse::<i64>() {
        Ok(value) => value <= 2 || cap_perfmon,
        Err(_) => false,
    }
}

#[derive(Deserialize)]
struct SetDatadogContinuousProfilerCanaryRequest {
    enabled: bool,
}

#[derive(Serialize)]
struct SetDatadogContinuousProfilerCanaryResponse {
    enabled: bool,
    service_id: String,
    variables_set: usize,
    app_redeploy_triggered: bool,
}

async fn set_datadog_continuous_profiler_canary(
    State(state): State<SetupApiState>,
    Json(req): Json<SetDatadogContinuousProfilerCanaryRequest>,
) -> impl IntoResponse {
    let vault = match state.platform.server.secrets_vault.as_ref() {
        Some(v) => v,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Vault not initialized" })),
            )
                .into_response();
        }
    };

    let railway_token = vault.get_secret(&state.tenant, "railway_token");
    let project_id = vault.get_secret(&state.tenant, "railway_project_id");
    let environment_id = vault.get_secret(&state.tenant, "railway_environment_id");
    let app_service_id = vault.get_secret(&state.tenant, "railway_service_id");

    let (Some(token), Some(project), Some(env), Some(app_svc)) =
        (railway_token, project_id, environment_id, app_service_id)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Railway continuous profiler canary requires railway_token, railway_project_id, railway_environment_id, and railway_service_id."
            })),
        )
            .into_response();
    };

    let value = if req.enabled { "true" } else { "false" }.to_string();
    let variables = vec![
        ("TEMPER_DDPROF_ENABLED", value.clone()),
        ("DD_PROFILING_ENABLED", value),
    ];

    let client = reqwest::Client::new();
    let result = async {
        for (name, value) in &variables {
            railway_upsert_variable(&client, &token, &project, &env, &app_svc, name, value).await?;
        }
        railway_redeploy_service(&client, &token, &env, &app_svc).await?;

        Ok::<SetDatadogContinuousProfilerCanaryResponse, anyhow::Error>(
            SetDatadogContinuousProfilerCanaryResponse {
                enabled: req.enabled,
                service_id: app_svc,
                variables_set: variables.len(),
                app_redeploy_triggered: true,
            },
        )
    }
    .await;

    match result {
        Ok(response) => (StatusCode::OK, Json(serde_json::json!(response))).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct EmitDatadogErrorTrackingSyntheticRequest {
    proof_id: Option<String>,
}

#[derive(Serialize)]
struct EmitDatadogErrorTrackingSyntheticResponse {
    emitted: bool,
    proof_id: String,
    service: String,
    env: String,
    version: String,
    error_type: &'static str,
    error_message: String,
    required_fields: Vec<&'static str>,
}

const DATADOG_ERROR_TRACKING_REQUIRED_FIELDS: [&str; 7] = [
    "error.type",
    "error.kind",
    "error.message",
    "error.stack",
    "exception.type",
    "exception.message",
    "exception.stacktrace",
];

async fn emit_datadog_error_tracking_synthetic(
    State(state): State<SetupApiState>,
    Json(req): Json<EmitDatadogErrorTrackingSyntheticRequest>,
) -> impl IntoResponse {
    let proof_id = req
        .proof_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| {
            let epoch_seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            format!("dd-error-tracking-{epoch_seconds}")
        });
    let service = env::var("DD_SERVICE").unwrap_or_else(|_| "temperpaw".to_string());
    let env_name = env::var("DD_ENV").unwrap_or_else(|_| "prod".to_string());
    let version = env::var("DD_VERSION").unwrap_or_else(|_| state.build_sha.clone());
    let error_type = "DatadogSyntheticBackendError";
    let error_message =
        format!("Synthetic Datadog Error Tracking backend issue for proof {proof_id}");
    let error_stack = format!(
        "{error_type}: {error_message}\n  at emit_datadog_error_tracking_synthetic (crates/temperpaw/src/setup_api.rs:1)\n  at railway_datadog_product_coverage_proof (docs/adrs/0049-railway-datadog-product-coverage.md:1)"
    );

    let tracer = global::tracer("temperpaw.setup_api");
    let mut span = tracer
        .span_builder("datadog.error_tracking.synthetic")
        .with_kind(SpanKind::Internal)
        .with_status(Status::error(error_message.clone()))
        .with_attributes(vec![
            KeyValue::new("service.name", service.clone()),
            KeyValue::new("deployment.environment.name", env_name.clone()),
            KeyValue::new("env", env_name.clone()),
            KeyValue::new("service.version", version.clone()),
            KeyValue::new("version", version.clone()),
            KeyValue::new("proof_id", proof_id.clone()),
            KeyValue::new("datadog.error_tracking.synthetic", true),
            KeyValue::new("error.type", error_type),
            KeyValue::new("error.kind", error_type),
            KeyValue::new("error.message", error_message.clone()),
            KeyValue::new("error.stack", error_stack.clone()),
            KeyValue::new("exception.type", error_type),
            KeyValue::new("exception.message", error_message.clone()),
            KeyValue::new("exception.stacktrace", error_stack.clone()),
        ])
        .start(&tracer);
    span.add_event(
        "exception",
        vec![
            KeyValue::new("exception.type", error_type),
            KeyValue::new("exception.message", error_message.clone()),
            KeyValue::new("exception.stacktrace", error_stack.clone()),
            KeyValue::new("error.type", error_type),
            KeyValue::new("error.message", error_message.clone()),
            KeyValue::new("error.stack", error_stack.clone()),
        ],
    );
    span.end();

    tracing::error!(
        target: "temperpaw.datadog.error_tracking",
        source = "custom",
        ddsource = "rust",
        service.name = %service,
        env = %env_name,
        version = %version,
        proof_id = %proof_id,
        datadog.error_tracking.synthetic = true,
        error.r#type = %error_type,
        error.kind = %error_type,
        error.message = %error_message,
        error.stack = %error_stack,
        exception.r#type = %error_type,
        exception.message = %error_message,
        exception.stacktrace = %error_stack,
        "DatadogSyntheticBackendError: synthetic backend error emitted for Datadog Error Tracking proof"
    );

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!(
            EmitDatadogErrorTrackingSyntheticResponse {
                emitted: true,
                proof_id,
                service,
                env: env_name,
                version,
                error_type,
                error_message,
                required_fields: DATADOG_ERROR_TRACKING_REQUIRED_FIELDS.to_vec(),
            }
        )),
    )
}

fn datadog_runtime_agent_railway_vars(
    dd_api_key: &str,
    dd_site: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("DD_API_KEY", dd_api_key.to_string()),
        ("DD_SITE", dd_site.to_string()),
        ("DD_ENV", "prod".to_string()),
        ("DD_SERVICE", "temperpaw".to_string()),
        ("DD_HOSTNAME", "temperpaw-runtime-agent".to_string()),
        (
            "DD_TAGS",
            "team:temperpaw service:temperpaw railway_profile:datadog-enhanced".to_string(),
        ),
        ("DD_APM_ENABLED", "true".to_string()),
        ("DD_APM_NON_LOCAL_TRAFFIC", "true".to_string()),
        (
            "DD_APM_FEATURES",
            "enable_operation_and_resource_name_logic_v2".to_string(),
        ),
        ("DD_LOGS_ENABLED", "true".to_string()),
        ("DD_OTLP_CONFIG_LOGS_ENABLED", "true".to_string()),
        (
            "DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_HTTP_ENDPOINT",
            "0.0.0.0:4318".to_string(),
        ),
        (
            "DD_OTLP_CONFIG_RECEIVER_PROTOCOLS_GRPC_ENDPOINT",
            "0.0.0.0:4317".to_string(),
        ),
        ("DD_PROCESS_AGENT_ENABLED", "true".to_string()),
    ]
}

fn datadog_enhanced_app_railway_vars(
    dd_api_key: &str,
    dd_site: &str,
    build_sha: &str,
) -> Vec<(&'static str, String)> {
    let version = if build_sha.trim().is_empty() {
        "unknown".to_string()
    } else {
        build_sha.to_string()
    };
    let otel_resource_attributes = datadog_app_otel_resource_attributes(&version);

    vec![
        ("DD_API_KEY", dd_api_key.to_string()),
        ("DD_SITE", dd_site.to_string()),
        ("DD_SERVICE", "temperpaw".to_string()),
        ("DD_ENV", "prod".to_string()),
        ("DD_VERSION", version),
        ("DD_TAGS", "team:temperpaw".to_string()),
        ("TEMPER_PROFILING_ENABLED", "true".to_string()),
        ("TEMPER_PROFILING_AUTO_UPLOAD", "true".to_string()),
        (
            "TEMPER_DATADOG_RAILWAY_PROFILE",
            "datadog-enhanced-railway".to_string(),
        ),
        ("DD_LLMOBS_ENABLED", "true".to_string()),
        ("DD_LLMOBS_API_ENABLED", "true".to_string()),
        ("OTEL_RESOURCE_ATTRIBUTES", otel_resource_attributes),
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            format!("http://{DATADOG_RUNTIME_AGENT_HOST}:4318"),
        ),
        ("DD_AGENT_HOST", DATADOG_RUNTIME_AGENT_HOST.to_string()),
        ("DD_TRACE_AGENT_PORT", "8126".to_string()),
        (
            "DD_TRACE_AGENT_URL",
            format!("http://{DATADOG_RUNTIME_AGENT_HOST}:8126"),
        ),
    ]
}

fn datadog_app_otel_resource_attributes(version: &str) -> String {
    format!(
        "service.name=temperpaw,service.version={version},deployment.environment=prod,dd_llmobs_enabled=false"
    )
}

async fn railway_find_service_by_name(
    client: &reqwest::Client,
    token: &str,
    project_id: &str,
    name: &str,
) -> Result<Option<String>> {
    let query = serde_json::json!({
        "query": "query($projectId: String!) { project(id: $projectId) { services { edges { node { id name } } } } }",
        "variables": { "projectId": project_id },
    });
    let data = railway_graphql_data(client, token, query, "Runtime Agent service lookup").await?;
    let services = data
        .pointer("/project/services/edges")
        .and_then(|edges| edges.as_array())
        .ok_or_else(|| anyhow!("Railway project service list was missing from API response"))?;

    Ok(services.iter().find_map(|edge| {
        let node = edge.get("node")?;
        let service_name = node.get("name")?.as_str()?;
        if service_name == name {
            node.get("id")?.as_str().map(str::to_string)
        } else {
            None
        }
    }))
}

async fn railway_create_datadog_runtime_agent_service(
    client: &reqwest::Client,
    token: &str,
    project_id: &str,
    environment_id: &str,
    dd_api_key: &str,
    dd_site: &str,
) -> Result<String> {
    let variables = datadog_runtime_agent_railway_vars(dd_api_key, dd_site)
        .into_iter()
        .map(|(name, value)| (name.to_string(), serde_json::Value::String(value)))
        .collect::<serde_json::Map<_, _>>();
    let query = serde_json::json!({
        "query": "mutation($projectId: String!, $environmentId: String!, $name: String!, $source: ServiceSourceInput, $variables: EnvironmentVariables) { serviceCreate(input: { projectId: $projectId, environmentId: $environmentId, name: $name, source: $source, variables: $variables }) { id name } }",
        "variables": {
            "projectId": project_id,
            "environmentId": environment_id,
            "name": DATADOG_RUNTIME_AGENT_SERVICE_NAME,
            "source": { "image": DATADOG_RUNTIME_AGENT_IMAGE },
            "variables": variables,
        },
    });
    let data = railway_graphql_data(client, token, query, "Runtime Agent serviceCreate").await?;
    data.pointer("/serviceCreate/id")
        .and_then(|id| id.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Railway serviceCreate response did not include a service id"))
}

async fn railway_update_service_source_image(
    client: &reqwest::Client,
    token: &str,
    environment_id: &str,
    service_id: &str,
    image: &str,
) -> Result<()> {
    let query = serde_json::json!({
        "query": "mutation($serviceId: String!, $environmentId: String!, $input: ServiceInstanceUpdateInput!) { serviceInstanceUpdate(serviceId: $serviceId, environmentId: $environmentId, input: $input) }",
        "variables": {
            "serviceId": service_id,
            "environmentId": environment_id,
            "input": {
                "source": { "image": image },
                "restartPolicyType": "ALWAYS",
                "numReplicas": 1,
            },
        },
    });
    let _ =
        railway_graphql_data(client, token, query, "Runtime Agent serviceInstanceUpdate").await?;
    Ok(())
}

async fn railway_upsert_variable(
    client: &reqwest::Client,
    token: &str,
    project_id: &str,
    environment_id: &str,
    service_id: &str,
    name: &str,
    value: &str,
) -> Result<()> {
    let query = serde_json::json!({
        "query": "mutation($input: VariableUpsertInput!) { variableUpsert(input: $input) }",
        "variables": {
            "input": {
                "projectId": project_id,
                "environmentId": environment_id,
                "serviceId": service_id,
                "name": name,
                "value": value,
                "skipDeploys": true,
            }
        }
    });
    let _ = railway_graphql_data(client, token, query, "Runtime Agent variableUpsert").await?;
    Ok(())
}

async fn railway_redeploy_service(
    client: &reqwest::Client,
    token: &str,
    environment_id: &str,
    service_id: &str,
) -> Result<()> {
    let query = match railway_latest_deployment_id(client, RAILWAY_GRAPHQL_URL, token, service_id)
        .await
    {
        Ok(deployment_id) => serde_json::json!({
            "query": "mutation($deploymentId: String!) { deploymentRedeploy(id: $deploymentId) { id status } }",
            "variables": { "deploymentId": deployment_id },
        }),
        Err(lookup_error) => {
            tracing::warn!(
                %service_id,
                %lookup_error,
                "Railway service has no latest deployment; triggering a fresh deploy"
            );
            serde_json::json!({
                "query": "mutation($serviceId: String!, $environmentId: String!) { serviceInstanceDeployV2(serviceId: $serviceId, environmentId: $environmentId) }",
                "variables": {
                    "serviceId": service_id,
                    "environmentId": environment_id,
                }
            })
        }
    };
    let _ = railway_graphql_data(client, token, query, "Runtime Agent redeploy").await?;
    Ok(())
}

async fn railway_graphql_data(
    client: &reqwest::Client,
    token: &str,
    body: serde_json::Value,
    operation: &str,
) -> Result<serde_json::Value> {
    let body = railway_graphql(client, RAILWAY_GRAPHQL_URL, token, body, operation)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(body.get("data").cloned().unwrap_or_default())
}

async fn persist_infra_secret(
    vault: &Arc<temper_server::secrets::SecretsVault>,
    storage: &PawStorage,
    tenant: &str,
    key: &str,
    value: &str,
) {
    let _ = vault.cache_secret(tenant, key, value.to_string());
    let _ = vault.cache_platform_secret(key, value.to_string());
    if let Ok((ciphertext, nonce)) = vault.encrypt(value.as_bytes()) {
        let _ = storage
            .upsert_secret(tenant, key, &ciphertext, &nonce)
            .await;
    }
}

// ────────────────────────────── Version + Updates ─────────────────────────────

#[derive(Serialize)]
struct VersionInfo {
    version: String,
    sha: String,
}

async fn get_version(State(state): State<SetupApiState>) -> Json<VersionInfo> {
    Json(VersionInfo {
        version: state.build_version.clone(),
        sha: state.build_sha.clone(),
    })
}

#[derive(Serialize)]
struct UpdateCheck {
    current_version: String,
    latest_version: Option<String>,
    latest_sha: Option<String>,
    update_available: bool,
    release_url: Option<String>,
    release_notes: Option<String>,
}

async fn check_for_updates(State(state): State<SetupApiState>) -> impl IntoResponse {
    let current = &state.build_version;

    let client = reqwest::Client::builder()
        .user_agent("temperpaw-server")
        .build()
        .unwrap_or_default();

    match client
        .get("https://api.github.com/repos/nerdsane/temperpaw/releases/latest")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let tag = body["tag_name"].as_str().unwrap_or("").to_string();
            let html_url = body["html_url"].as_str().map(|s| s.to_string());
            let notes = body["body"].as_str().map(|s| {
                // Truncate release notes to first 500 chars for the API response
                if s.len() > 500 {
                    format!("{}...", &s[..500])
                } else {
                    s.to_string()
                }
            });

            // If current is "dev" or "sha-*" (non-release build), any release is an update.
            // If current is a release tag (e.g. "v0.2.0"), compare against latest tag.
            let update_available = if tag.is_empty() {
                false
            } else if current == "dev" || current.starts_with("sha-") || current == "unknown" {
                true
            } else {
                tag != *current
            };

            (
                StatusCode::OK,
                Json(serde_json::json!(UpdateCheck {
                    current_version: current.clone(),
                    latest_version: if tag.is_empty() { None } else { Some(tag) },
                    latest_sha: None,
                    update_available,
                    release_url: html_url,
                    release_notes: notes,
                })),
            )
                .into_response()
        }
        Ok(resp) => {
            let status = resp.status();
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("GitHub API returned {status}"),
                    "current_version": current,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": format!("Failed to check GitHub releases: {e}"),
                "current_version": current,
            })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct EdgeBuild {
    available: bool,
    sha: Option<String>,
    short_sha: Option<String>,
    message: Option<String>,
    committed_at: Option<String>,
}

async fn check_edge_build(_state: State<SetupApiState>) -> impl IntoResponse {
    let client = reqwest::Client::builder()
        .user_agent("temperpaw-server")
        .build()
        .unwrap_or_default();

    match client
        .get("https://api.github.com/repos/nerdsane/temperpaw/commits/main")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let sha = body["sha"].as_str().map(|s| s.to_string());
            let short_sha = sha.as_deref().map(|s| s[..7.min(s.len())].to_string());
            let message = body["commit"]["message"]
                .as_str()
                .map(|s| s.lines().next().unwrap_or(s).to_string());
            let committed_at = body["commit"]["committer"]["date"]
                .as_str()
                .map(|s| s.to_string());

            (
                StatusCode::OK,
                Json(serde_json::json!(EdgeBuild {
                    available: sha.is_some(),
                    sha,
                    short_sha,
                    message,
                    committed_at,
                })),
            )
                .into_response()
        }
        Ok(resp) => {
            let status = resp.status();
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("GitHub API returned {status}") })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Failed to check edge build: {e}") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct RedeployRequest {
    /// Optional image tag to deploy. "edge" for latest main build, "latest" for latest release.
    /// If omitted, redeploys with the current configuration (no tag change).
    image_tag: Option<String>,
    /// Optional full git SHA for the selected image. When present, Railway runtime version
    /// variables are aligned before redeploy so /paw/version proves the running build.
    build_sha: Option<String>,
}

async fn railway_redeploy(
    State(state): State<SetupApiState>,
    Json(req): Json<RedeployRequest>,
) -> impl IntoResponse {
    // Only allow known tags to prevent arbitrary image injection
    if let Some(ref tag) = req.image_tag
        && tag != "latest"
        && tag != "edge"
        && !is_sha_image_tag(tag)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "image_tag must be 'latest', 'edge', or 'sha-*'" })),
        )
            .into_response();
    }
    if let Some(ref build_sha) = req.build_sha
        && (build_sha.len() != 40 || !build_sha.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": "build_sha must be a full 40-character hex git SHA" }),
            ),
        )
            .into_response();
    }

    let vault = match state.platform.server.secrets_vault.as_ref() {
        Some(v) => v,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Vault not initialized" })),
            )
                .into_response();
        }
    };

    let railway_token = vault.get_secret(&state.tenant, "railway_token");
    let project_id = vault.get_secret(&state.tenant, "railway_project_id");
    let environment_id = vault.get_secret(&state.tenant, "railway_environment_id");
    let service_id = vault.get_secret(&state.tenant, "railway_service_id");

    let (Some(token), Some(project), Some(env), Some(svc)) =
        (railway_token, project_id, environment_id, service_id)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Railway integration not configured. Set railway_token, railway_project_id, railway_environment_id, and railway_service_id."
            })),
        )
            .into_response();
    };

    let client = reqwest::Client::new();
    let railway_url = "https://backboard.railway.com/graphql/v2";

    // If an image_tag was requested, set it as a Railway variable first.
    // The deploy Dockerfile uses `ARG IMAGE_TAG=latest` so this controls which image is pulled.
    if let Some(ref tag) = req.image_tag {
        let image_tag_result =
            railway_upsert_variable(&client, &token, &project, &env, &svc, "IMAGE_TAG", tag).await;
        if let Err(error) = image_tag_result {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    }

    let build_version = req
        .build_sha
        .as_ref()
        .map(|build_sha| format!("sha-{}", &build_sha[..8]));
    let deployment_runtime_vars = match (&req.build_sha, &build_version) {
        (Some(build_sha), Some(build_version)) => vec![
            ("BUILD_SHA", build_sha.clone()),
            ("BUILD_VERSION", build_version.clone()),
            ("DD_VERSION", build_version.clone()),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                datadog_app_otel_resource_attributes(build_version),
            ),
        ],
        _ => Vec::new(),
    };
    for (name, value) in deployment_runtime_vars {
        if let Err(error) =
            railway_upsert_variable(&client, &token, &project, &env, &svc, name, &value).await
        {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    }

    let deployment_id = match railway_latest_deployment_id(&client, railway_url, &token, &svc).await
    {
        Ok(id) => id,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": error })),
            )
                .into_response();
        }
    };

    let redeploy_query = serde_json::json!({
        "query": "mutation($deploymentId: String!) { deploymentRedeploy(id: $deploymentId) { id status } }",
        "variables": { "deploymentId": deployment_id },
    });

    match railway_graphql(&client, railway_url, &token, redeploy_query, "redeploy").await {
        Ok(body) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "triggered": true,
                "image_tag": req.image_tag.as_deref().unwrap_or("current"),
                "build_sha": req.build_sha.as_deref(),
                "deployment_id": deployment_id,
                "redeploy": body.get("data").and_then(|data| data.get("deploymentRedeploy")).cloned(),
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

fn is_sha_image_tag(tag: &str) -> bool {
    let Some(sha) = tag.strip_prefix("sha-") else {
        return false;
    };
    !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit())
}

async fn railway_graphql(
    client: &reqwest::Client,
    railway_url: &str,
    token: &str,
    payload: serde_json::Value,
    operation: &str,
) -> Result<serde_json::Value, String> {
    let resp = client
        .post(railway_url)
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Railway {operation} request failed: {e}"))?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if !status.is_success() || body.get("errors").is_some() {
        let error_msg = body["errors"]
            .as_array()
            .and_then(|errors| errors.first())
            .and_then(|error| error["message"].as_str())
            .unwrap_or("Railway API error");
        return Err(format!("Railway {operation} failed: {error_msg}"));
    }

    Ok(body)
}

async fn railway_latest_deployment_id(
    client: &reqwest::Client,
    railway_url: &str,
    token: &str,
    service_id: &str,
) -> Result<String, String> {
    let query = serde_json::json!({
        "query": "query($serviceId: String!) { service(id: $serviceId) { serviceInstances { edges { node { latestDeployment { id status createdAt } } } } } }",
        "variables": { "serviceId": service_id },
    });

    let body = railway_graphql(
        client,
        railway_url,
        token,
        query,
        "latest deployment lookup",
    )
    .await?;
    let edges = body
        .get("data")
        .and_then(|data| data.get("service"))
        .and_then(|service| service.get("serviceInstances"))
        .and_then(|instances| instances.get("edges"))
        .and_then(|edges| edges.as_array())
        .ok_or_else(|| {
            "Railway latest deployment lookup returned no service instances".to_string()
        })?;

    for edge in edges {
        if let Some(id) = edge
            .get("node")
            .and_then(|node| node.get("latestDeployment"))
            .and_then(|deployment| deployment.get("id"))
            .and_then(|id| id.as_str())
            .filter(|id| !id.is_empty())
        {
            return Ok(id.to_string());
        }
    }

    Err("Railway latest deployment lookup found no latestDeployment.id".to_string())
}

// ──────────────────────────────── Transports ─────────────────────────────────

async fn get_transport_status(State(state): State<SetupApiState>) -> Json<TransportStatusResponse> {
    let runtime = state.transport_manager.status().await;
    let vault = state.platform.server.secrets_vault.as_ref();
    let has_discord =
        secret_is_configured(vault.and_then(|v| v.get_secret(&state.tenant, "discord_bot_token")));
    let has_slack =
        secret_is_configured(vault.and_then(|v| v.get_secret(&state.tenant, "slack_bot_token")));
    let discord_connection = discord_transport_connection_snapshot(&state).await;

    Json(TransportStatusResponse {
        discord: transport_status_report_with_connection(
            has_discord,
            &runtime.discord,
            discord_connection.as_ref(),
        ),
        slack: transport_status_report(has_slack, &runtime.slack, None, None),
    })
}

pub(crate) async fn get_readyz(State(state): State<SetupApiState>) -> impl IntoResponse {
    let runtime = state.transport_manager.status().await;
    let vault = state.platform.server.secrets_vault.as_ref();
    let has_discord =
        secret_is_configured(vault.and_then(|v| v.get_secret(&state.tenant, "discord_bot_token")));
    let discord_connection = if has_discord {
        discord_transport_connection_snapshot(&state).await
    } else {
        None
    };
    let desired_state = discord_connection
        .as_ref()
        .and_then(|snapshot| field_str(&snapshot.fields, &["desired_state", "DesiredState"]));
    let connection_state = discord_connection
        .as_ref()
        .map(|snapshot| snapshot.status.as_str());
    let (status, mut body) = discord_readyz_response(
        has_discord,
        &runtime.discord,
        desired_state,
        connection_state,
    );

    if let Some(connection) = discord_connection.as_ref() {
        body["discord"]["last_error"] = field_str(&connection.fields, &["last_error", "LastError"])
            .or_else(|| field_str(&connection.fields, &["error_message", "ErrorMessage"]))
            .map(|value| serde_json::json!(value))
            .unwrap_or(serde_json::Value::Null);
        body["discord"]["next_retry_at"] =
            field_str(&connection.fields, &["next_retry_at", "NextRetryAt"])
                .map(|value| serde_json::json!(value))
                .unwrap_or(serde_json::Value::Null);
    }

    (status, Json(body))
}

async fn proxy_discord_interaction(
    State(state): State<SetupApiState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let signature = headers
        .get("x-signature-ed25519")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let timestamp = headers
        .get("x-signature-timestamp")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let vault = state.platform.server.secrets_vault.as_ref();
    let configured_public_key = vault
        .and_then(|vault| vault.get_secret(&state.tenant, "discord_public_key"))
        .filter(|value| !value.trim().is_empty());

    if !signature.is_empty() && !timestamp.is_empty() {
        let mut verified_public_key = configured_public_key
            .as_deref()
            .filter(|public_key| verify_discord_signature(public_key, signature, timestamp, &body))
            .map(str::to_string);

        if verified_public_key.is_none() {
            let bot_token =
                vault.and_then(|vault| vault.get_secret(&state.tenant, "discord_bot_token"));
            if let (Some(vault), Some(bot_token)) = (vault, bot_token) {
                match resolve_and_persist_discord_public_key(
                    vault,
                    &state.storage,
                    &state.tenant,
                    &bot_token,
                    configured_public_key.as_deref(),
                )
                .await
                {
                    Ok(refreshed_public_key)
                        if verify_discord_signature(
                            &refreshed_public_key,
                            signature,
                            timestamp,
                            &body,
                        ) =>
                    {
                        if configured_public_key.as_deref() != Some(refreshed_public_key.as_str()) {
                            tracing::info!(
                                "Healed stale Discord verify_key during public endpoint verification"
                            );
                        }
                        verified_public_key = Some(refreshed_public_key);
                    }
                    Ok(_) | Err(_) => {}
                }
            }
        }

        if verified_public_key.is_none() {
            tracing::warn!("Discord interaction signature verification failed at public endpoint");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid signature" })),
            )
                .into_response();
        }

        if is_discord_ping(&body) {
            tracing::info!("Discord interaction endpoint verification succeeded");
            return (StatusCode::OK, Json(serde_json::json!({ "type": 1 }))).into_response();
        }
    }

    let status = state.transport_manager.status().await;
    if !matches!(
        status.discord,
        crate::transport_manager::TransportStatus::Connected { .. }
            | crate::transport_manager::TransportStatus::Connecting
    ) {
        tracing::warn!("Discord interaction received while transport is unavailable");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "discord transport is not running" })),
        )
            .into_response();
    }

    let client = reqwest::Client::new();
    let mut request = client
        .post(state.transport_manager.discord_interaction_proxy_url())
        .body(body.to_vec());

    for header_name in [
        "content-type",
        "x-signature-ed25519",
        "x-signature-timestamp",
    ] {
        if let Some(value) = headers.get(header_name) {
            request = request.header(header_name, value.clone());
        }
    }

    match request.send().await {
        Ok(resp) => {
            let status = resp.status();
            let content_type = resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .cloned();
            let bytes = resp.bytes().await.unwrap_or_default();
            if !status.is_success() {
                tracing::warn!(
                    http_status = %status,
                    "Discord interaction proxy returned a non-success status"
                );
            }
            let mut response = axum::http::Response::builder().status(status);
            if let Some(content_type) = content_type {
                response = response.header(axum::http::header::CONTENT_TYPE, content_type);
            }
            response
                .body(Body::from(bytes))
                .unwrap_or_else(|_| axum::http::Response::new(Body::empty()))
        }
        Err(error) => {
            tracing::error!(%error, "Failed to proxy Discord interaction");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("failed to proxy discord interaction: {error}")
                })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn resolve_and_persist_discord_public_key(
    vault: &Arc<temper_server::secrets::SecretsVault>,
    storage: &PawStorage,
    tenant: &str,
    bot_token: &str,
    configured_public_key: Option<&str>,
) -> Result<String> {
    let public_key =
        crate::discord_app::resolve_verify_key(bot_token, configured_public_key).await?;
    persist_discord_public_key(vault, storage, tenant, &public_key).await;
    Ok(public_key)
}

async fn persist_discord_public_key(
    vault: &Arc<temper_server::secrets::SecretsVault>,
    storage: &PawStorage,
    tenant: &str,
    public_key: &str,
) {
    let _ = vault.cache_secret(tenant, "discord_public_key", public_key.to_string());
    if let Ok((ct, nc)) = vault.encrypt(public_key.as_bytes()) {
        let _ = storage
            .upsert_secret(tenant, "discord_public_key", &ct, &nc)
            .await;
    }
}

fn is_discord_ping(body: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct InteractionEnvelope {
        #[serde(rename = "type")]
        interaction_type: u8,
    }

    matches!(
        serde_json::from_slice::<InteractionEnvelope>(body),
        Ok(InteractionEnvelope {
            interaction_type: 1
        })
    )
}

fn verify_discord_signature(
    public_key_hex: &str,
    signature_hex: &str,
    timestamp: &str,
    body: &[u8],
) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let Ok(pk_bytes) = hex::decode(public_key_hex) else {
        return false;
    };
    let pk_bytes: [u8; 32] = match pk_bytes.try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pk_bytes) else {
        return false;
    };

    let Ok(sig_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    let sig_bytes: [u8; 64] = match sig_bytes.try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let signature = Signature::from_bytes(&sig_bytes);

    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    verifying_key.verify(&message, &signature).is_ok()
}

#[derive(Deserialize)]
struct InternalDiscordStartRequest {
    transport_connection_id: Option<String>,
}

async fn start_discord_internal(
    State(state): State<SetupApiState>,
    Json(req): Json<InternalDiscordStartRequest>,
) -> impl IntoResponse {
    if req
        .transport_connection_id
        .as_deref()
        .is_some_and(|id| !id.is_empty() && id != DISCORD_TRANSPORT_CONNECTION_ID)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "unknown Discord transport connection",
                "retryable": false
            })),
        )
            .into_response();
    }

    let params = match discord_connect_params_from_vault(&state) {
        Ok(Some(params)) => params,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "discord_bot_token is not configured",
                    "retryable": false
                })),
            )
                .into_response();
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "retryable": true
                })),
            )
                .into_response();
        }
    };

    match state.transport_manager.connect_discord(params).await {
        Ok(interaction_url) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "connected",
                "discord_interaction_url": interaction_url
            })),
        )
            .into_response(),
        Err(error) => {
            let error = error.to_string();
            let retryable = discord_start_error_is_retryable(&error);
            (
                discord_start_failure_status(&error),
                Json(serde_json::json!({
                    "status": "failed",
                    "error": error,
                    "retryable": retryable
                })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct DiscordConnectRequest {
    bot_token: String,
    public_key: Option<String>,
    guild_id: Option<String>,
    feed_channel_id: Option<String>,
    forum_channel_id: Option<String>,
}

async fn connect_discord(
    State(state): State<SetupApiState>,
    Json(req): Json<DiscordConnectRequest>,
) -> impl IntoResponse {
    let Some(vault) = state.platform.server.secrets_vault.as_ref() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "secrets vault is not configured"
            })),
        )
            .into_response();
    };
    let resolved_public_key = match resolve_and_persist_discord_public_key(
        vault,
        &state.storage,
        &state.tenant,
        &req.bot_token,
        req.public_key.as_deref(),
    )
    .await
    {
        Ok(public_key) => public_key,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": error.to_string()
                })),
            )
                .into_response();
        }
    };

    let _ = vault.cache_secret(&state.tenant, "discord_bot_token", req.bot_token.clone());
    if let Ok((ct, nc)) = vault.encrypt(req.bot_token.as_bytes()) {
        let _ = state
            .storage
            .upsert_secret(&state.tenant, "discord_bot_token", &ct, &nc)
            .await;
    }
    let _ = vault.cache_secret(
        &state.tenant,
        "discord_public_key",
        resolved_public_key.clone(),
    );

    for (key, value) in [
        ("discord_guild_id", req.guild_id.clone()),
        ("discord_feed_channel_id", req.feed_channel_id.clone()),
        ("discord_forum_channel_id", req.forum_channel_id.clone()),
    ] {
        if let Some(value) = value {
            let _ = vault.cache_secret(&state.tenant, key, value.clone());
            if let Ok((ct, nc)) = vault.encrypt(value.as_bytes()) {
                let _ = state
                    .storage
                    .upsert_secret(&state.tenant, key, &ct, &nc)
                    .await;
            }
        }
    }

    match schedule_discord_reconcile(&state.platform, &state.tenant).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "scheduled",
                "discord_interaction_url": state.transport_manager.discord_interaction_public_url().await
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "schedule_failed",
                "error": error.to_string(),
                "retryable": true
            })),
        )
            .into_response(),
    }
}

async fn disconnect_discord(State(state): State<SetupApiState>) -> Json<serde_json::Value> {
    state.transport_manager.disconnect_discord().await;
    Json(serde_json::json!({ "status": "disconnected" }))
}

#[derive(Deserialize)]
struct SlackConnectRequest {
    app_token: String,
    bot_token: String,
    signing_secret: Option<String>,
}

async fn connect_slack(
    State(state): State<SetupApiState>,
    Json(req): Json<SlackConnectRequest>,
) -> impl IntoResponse {
    // Save tokens to vault + Turso
    if let Some(vault) = state.platform.server.secrets_vault.as_ref() {
        let _ = vault.cache_secret(&state.tenant, "slack_bot_token", req.bot_token.clone());
        let _ = vault.cache_secret(&state.tenant, "slack_app_token", req.app_token.clone());
        if let Ok((ct, nc)) = vault.encrypt(req.bot_token.as_bytes()) {
            let _ = state
                .storage
                .upsert_secret(&state.tenant, "slack_bot_token", &ct, &nc)
                .await;
        }
        if let Ok((ct, nc)) = vault.encrypt(req.app_token.as_bytes()) {
            let _ = state
                .storage
                .upsert_secret(&state.tenant, "slack_app_token", &ct, &nc)
                .await;
        }
    }

    state
        .transport_manager
        .connect_slack(SlackConnectParams {
            app_token: req.app_token,
            bot_token: req.bot_token,
            signing_secret: req.signing_secret.unwrap_or_default(),
        })
        .await;

    Json(serde_json::json!({ "status": "connecting" }))
}

async fn disconnect_slack(State(state): State<SetupApiState>) -> Json<serde_json::Value> {
    state.transport_manager.disconnect_slack().await;
    Json(serde_json::json!({ "status": "disconnected" }))
}

#[cfg(test)]
mod tests {
    use super::{
        InstallFromGenesisRequest, allowed_secret_keys, datadog_enhanced_app_railway_vars,
        datadog_runtime_agent_railway_vars, discord_connect_params_for_secret_update,
        discord_readyz_response, discord_start_error_is_retryable,
        genesis_install_request_from_setup, is_discord_ping, persist_discord_public_key,
        personalized_soul_flag_value, secrets_schema, transport_status_report,
        validate_setup_secret_key, verify_discord_signature,
    };
    use crate::transport_manager::TransportStatus;
    use axum::http::StatusCode;
    use ed25519_dalek::{Signer, SigningKey};
    use std::sync::Arc;
    use temper_server::secrets::SecretsVault;
    use temper_store_turso::TursoEventStore;

    fn genesis_install_setup_request(
        app_ref: &str,
        registry_url: &str,
        registry_tenant: &str,
        follow_policy: &str,
    ) -> InstallFromGenesisRequest {
        InstallFromGenesisRequest {
            app_ref: app_ref.to_string(),
            registry_url: registry_url.to_string(),
            registry_tenant: registry_tenant.to_string(),
            follow_policy: follow_policy.to_string(),
        }
    }

    #[test]
    fn genesis_install_request_from_setup_maps_valid_request() {
        let request = genesis_install_request_from_setup(
            "tenant-a",
            genesis_install_setup_request(
                " katagami/katagami-curation@sha-123 ",
                "https://genesis.example.test/",
                " registry-team ",
                "follow-latest",
            ),
        )
        .expect("valid pinned Genesis setup request");

        assert_eq!(request.tenant, "tenant-a");
        assert_eq!(request.app_ref, "katagami/katagami-curation@sha-123");
        assert_eq!(request.registry_url, "https://genesis.example.test");
        assert_eq!(request.registry_tenant, "registry-team");
        assert_eq!(request.follow_policy, "follow_latest");
    }

    #[test]
    fn genesis_install_request_from_setup_defaults_tenant_and_policy() {
        let request = genesis_install_request_from_setup(
            "tenant-a",
            genesis_install_setup_request(
                "katagami/katagami-curation@sha-123",
                "https://genesis.example.test",
                "",
                "",
            ),
        )
        .expect("valid pinned Genesis setup request");

        assert_eq!(request.registry_tenant, "default");
        assert_eq!(request.follow_policy, "pinned");
    }

    #[test]
    fn genesis_install_request_from_setup_rejects_unpinned_refs() {
        let error = genesis_install_request_from_setup(
            "tenant-a",
            genesis_install_setup_request(
                "katagami/katagami-curation",
                "https://genesis.example.test",
                "default",
                "pinned",
            ),
        )
        .expect_err("unpinned Genesis ref must fail at setup boundary");

        assert!(error.contains("owner/name@hash"));
    }

    #[test]
    fn genesis_install_request_from_setup_rejects_bad_registry_url_and_policy() {
        let bad_url = genesis_install_request_from_setup(
            "tenant-a",
            genesis_install_setup_request(
                "katagami/katagami-curation@sha-123",
                "genesis.example.test",
                "default",
                "pinned",
            ),
        )
        .expect_err("registry URL must include scheme");
        assert!(bad_url.contains("registry_url"));

        let bad_policy = genesis_install_request_from_setup(
            "tenant-a",
            genesis_install_setup_request(
                "katagami/katagami-curation@sha-123",
                "https://genesis.example.test",
                "default",
                "latest",
            ),
        )
        .expect_err("unsupported follow policy must fail at setup boundary");
        assert!(bad_policy.contains("follow_policy"));
    }

    #[test]
    fn genesis_token_is_a_known_integration_secret() {
        // Genesis push credential for Paw agents: it must be both an allowed
        // secret key and present in the setup schema (mirrors github_token), so
        // `temper.get_secret("genesis_token")` resolves the seeded value.
        assert!(
            allowed_secret_keys().contains(&"genesis_token"),
            "genesis_token must be an allowed secret key"
        );
        assert!(
            secrets_schema().iter().any(|s| s.key == "genesis_token"),
            "genesis_token must appear in the setup secrets schema"
        );
    }

    #[test]
    fn discord_secret_update_builds_reconnect_params_when_config_is_complete() {
        let params = discord_connect_params_for_secret_update(
            |key| match key {
                "discord_public_key" => Some("pub-key".to_string()),
                "discord_guild_id" => Some("guild-123".to_string()),
                _ => None,
            },
            "discord_bot_token",
            "new-token",
        )
        .expect("discord reconnect params should be built");

        assert_eq!(params.bot_token, "new-token");
        assert_eq!(params.public_key.as_deref(), Some("pub-key"));
        assert_eq!(params.guild_id.as_deref(), Some("guild-123"));
        assert_eq!(params.feed_channel_id, None);
        assert_eq!(params.forum_channel_id, None);
    }

    #[test]
    fn discord_secret_update_skips_reconnect_for_non_discord_secret_updates() {
        let params =
            discord_connect_params_for_secret_update(|_| None, "openai_api_key", "new-token");

        assert!(params.is_none());
    }

    #[test]
    fn discord_secret_update_reconnects_without_a_manual_public_key() {
        let params = discord_connect_params_for_secret_update(
            |key| match key {
                "discord_guild_id" => Some("guild-123".to_string()),
                _ => None,
            },
            "discord_bot_token",
            "new-token",
        )
        .expect("discord reconnect params should be built");

        assert_eq!(params.bot_token, "new-token");
        assert_eq!(params.public_key, None);
        assert_eq!(params.guild_id.as_deref(), Some("guild-123"));
    }

    #[test]
    fn discord_start_error_retryability_detects_local_odata_503() {
        assert!(discord_start_error_is_retryable(
            "create Channels returned 503 Service Unavailable: "
        ));
        assert!(discord_start_error_is_retryable(
            "Timed out waiting for Discord to reach READY"
        ));
        assert!(!discord_start_error_is_retryable(
            "Discord requires a public interactions URL"
        ));
        assert!(!discord_start_error_is_retryable(
            "Discord API returned 400 Bad Request"
        ));
    }

    #[test]
    fn discord_readyz_reports_degraded_without_changing_liveness() {
        let (status, body) = discord_readyz_response(
            true,
            &TransportStatus::Disconnected,
            Some("connected"),
            Some("Retrying"),
        );

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "degraded");
        assert_eq!(body["healthz"], "/healthz");
        assert_eq!(body["discord"]["configured"], true);
        assert_eq!(body["discord"]["connected"], false);
        assert_eq!(body["discord"]["desired_state"], "connected");
        assert_eq!(body["discord"]["connection_state"], "Retrying");

        let (status, body) =
            discord_readyz_response(false, &TransportStatus::Disconnected, None, None);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["discord"]["configured"], false);
    }

    #[test]
    fn transport_status_report_distinguishes_configured_from_connected() {
        let report = transport_status_report(
            true,
            &TransportStatus::Error {
                message: "create Channels returned 503 Service Unavailable: ".to_string(),
            },
            Some("connected"),
            Some("Retrying"),
        );

        assert_eq!(report.status, "error");
        assert!(report.configured);
        assert!(!report.connected);
        assert_eq!(report.desired_state.as_deref(), Some("connected"));
        assert_eq!(report.connection_state.as_deref(), Some("Retrying"));
        assert_eq!(
            report.message.as_deref(),
            Some("create Channels returned 503 Service Unavailable: ")
        );
    }

    #[test]
    fn personalized_soul_flag_value_accepts_truthy_values() {
        assert!(personalized_soul_flag_value(Some("true")));
        assert!(personalized_soul_flag_value(Some("1")));
        assert!(personalized_soul_flag_value(Some("yes")));
        assert!(!personalized_soul_flag_value(Some("false")));
        assert!(!personalized_soul_flag_value(None));
    }

    #[test]
    fn discord_signature_verification_accepts_valid_signature() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let body = br#"{"type":1}"#;
        let timestamp = "1744848000";
        let mut message = Vec::from(timestamp.as_bytes());
        message.extend_from_slice(body);
        let signature = signing_key.sign(&message);
        let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let signature_hex = hex::encode(signature.to_bytes());

        assert!(verify_discord_signature(
            &public_key_hex,
            &signature_hex,
            timestamp,
            body,
        ));
    }

    #[test]
    fn discord_ping_detection_identifies_ping_payloads() {
        assert!(is_discord_ping(br#"{"type":1}"#));
        assert!(!is_discord_ping(br#"{"type":3}"#));
        assert!(!is_discord_ping(br#"not-json"#));
    }

    #[tokio::test]
    async fn discord_public_key_persistence_updates_vault_and_turso() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let db_path = tempdir.path().join("setup-api.db");
        let turso_store = TursoEventStore::new(&format!("file:{}", db_path.display()), None)
            .await
            .expect("local turso store");
        let vault = Arc::new(SecretsVault::new(&[9u8; 32]));
        let tenant = "default";
        let refreshed_public_key =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let storage = crate::storage::PawStorage::from(turso_store.clone());

        persist_discord_public_key(&vault, &storage, tenant, refreshed_public_key).await;
        assert_eq!(
            vault.get_secret(tenant, "discord_public_key").as_deref(),
            Some(refreshed_public_key)
        );

        let rows = turso_store
            .load_secrets_for_tenant(tenant)
            .await
            .expect("load tenant secrets");
        let stored_public_key = rows
            .into_iter()
            .find_map(|(key_name, ciphertext, nonce)| {
                (key_name == "discord_public_key").then(|| {
                    vault
                        .decrypt(&ciphertext, &nonce)
                        .expect("decrypt public key")
                })
            })
            .expect("persisted public key");
        assert_eq!(
            String::from_utf8(stored_public_key).expect("utf8 public key"),
            refreshed_public_key
        );
    }

    #[test]
    fn modal_bridge_url_remains_internal_only() {
        assert!(allowed_secret_keys().contains("modal_bridge_url"));
        assert!(
            !secrets_schema()
                .iter()
                .any(|secret| secret.key == "modal_bridge_url"),
            "modal_bridge_url should be provisioned by deploy, not shown in the dashboard schema"
        );
    }

    #[test]
    fn datadog_agent_env_tags_use_datadog_whitespace_separator() {
        let runtime_agent_vars = datadog_runtime_agent_railway_vars("api-key", "datadoghq.com");
        let runtime_agent_tags = runtime_agent_vars
            .iter()
            .find_map(|(name, value)| (*name == "DD_TAGS").then_some(value.as_str()))
            .expect("runtime agent DD_TAGS must be set");

        assert_eq!(
            runtime_agent_tags,
            "team:temperpaw service:temperpaw railway_profile:datadog-enhanced"
        );
        assert!(
            !runtime_agent_tags.contains(','),
            "Datadog Agent DD_TAGS uses whitespace-separated list values"
        );

        let app_vars = datadog_enhanced_app_railway_vars("api-key", "datadoghq.com", "build-sha");
        let app_tags = app_vars
            .iter()
            .find_map(|(name, value)| (*name == "DD_TAGS").then_some(value.as_str()))
            .expect("app DD_TAGS must be set");
        assert_eq!(app_tags, "team:temperpaw");
        assert!(
            !app_tags.contains(','),
            "TemperPaw app DD_TAGS must also remain whitespace-safe"
        );
    }

    #[test]
    fn datadog_enhanced_app_vars_disable_otel_llmobs_auto_conversion() {
        let app_vars = datadog_enhanced_app_railway_vars("api-key", "datadoghq.com", "build-sha");
        let resource_attributes = app_vars
            .iter()
            .find_map(|(name, value)| (*name == "OTEL_RESOURCE_ATTRIBUTES").then_some(value))
            .expect("enhanced Datadog app vars must set OTEL_RESOURCE_ATTRIBUTES");

        assert_eq!(
            resource_attributes,
            "service.name=temperpaw,service.version=build-sha,deployment.environment=prod,dd_llmobs_enabled=false"
        );
    }

    #[test]
    fn openai_codex_canonical_secret_keys_are_allowed() {
        let allowed = allowed_secret_keys();
        for key in [
            "openai_codex_access_token",
            "openai_codex_refresh_token",
            "openai_codex_expires_at_ms",
            "openai_codex_account_id",
        ] {
            assert!(
                allowed.contains(key),
                "{key} must be settable by Codex auth flow"
            );
        }
    }

    #[test]
    fn open_weight_llm_secret_keys_are_allowed_and_rendered() {
        let allowed = allowed_secret_keys();
        let schema = secrets_schema();

        for key in [
            "openrouter_api_key",
            "openrouter_api_url",
            "huggingface_api_key",
            "hf_token",
            "huggingface_api_url",
            "fireworks_api_key",
            "fireworks_api_url",
            "sakana_fugu_api_key",
            "sakana_fugu_api_url",
            "openai_compatible_api_key",
            "openai_compatible_api_url",
            "openai_compatible_headers_json",
            "local_openai_api_url",
        ] {
            assert!(
                allowed.contains(key),
                "{key} should be accepted by setup API"
            );
        }

        for key in [
            "huggingface_api_key",
            "fireworks_api_key",
            "sakana_fugu_api_url",
            "openai_compatible_api_url",
            "local_openai_api_url",
        ] {
            assert!(
                schema.iter().any(|secret| secret.key == key),
                "{key} should be visible in dashboard schema"
            );
        }
    }

    #[test]
    fn setup_api_accepts_safe_custom_secret_names() {
        assert!(validate_setup_secret_key("vendor_x_api_key").is_ok());
        assert!(validate_setup_secret_key("team.provider:header").is_ok());
        assert!(validate_setup_secret_key("").is_err());
        assert!(validate_setup_secret_key("../oops").is_err());
        assert!(validate_setup_secret_key(" bad").is_err());
        assert!(validate_setup_secret_key("bad/key").is_err());
    }

    #[test]
    fn openai_codex_secret_schema_points_to_managed_oauth_not_codex_cli_import() {
        let codex = secrets_schema()
            .into_iter()
            .find(|secret| secret.key == "openai_codex_access_token")
            .expect("canonical Codex access token schema");

        assert_eq!(codex.label, "OpenAI Codex Access Token");
        assert!(codex.description.contains("TemperPaw-managed"));
        assert!(!codex.description.contains("~/.codex"));
    }
}
