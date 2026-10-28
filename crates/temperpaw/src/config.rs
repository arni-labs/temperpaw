//! Configuration loaded from environment variables.

use anyhow::{Context, anyhow};

/// Durable storage backend selected for Temper platform state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBackend {
    Turso,
    Postgres,
}

impl StorageBackend {
    pub fn from_env_value(name: &str, value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "turso" | "libsql" => Ok(Self::Turso),
            "postgres" | "postgresql" => Ok(Self::Postgres),
            other => Err(anyhow!(
                "{name} must be one of: postgres, turso; got {other:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Turso => "turso",
            Self::Postgres => "postgres",
        }
    }
}

/// Temper Paw daemon configuration.
pub struct Config {
    /// Discord bot token for the Paw agent.
    pub discord_bot_token: Option<String>,

    /// Discord application public key for interaction signature verification.
    pub discord_public_key: Option<String>,

    /// Discord guild (server) ID for observability channels.
    pub discord_guild_id: Option<String>,

    /// Discord text channel ID for the #feed stream.
    pub discord_feed_channel_id: Option<String>,

    /// Discord forum channel ID for per-agent threads.
    pub discord_forum_channel_id: Option<String>,

    /// Slack App-Level Token (xapp-...) for Socket Mode connection.
    pub slack_app_token: Option<String>,

    /// Slack Bot Token (xoxb-...) for Web API calls.
    pub slack_bot_token: Option<String>,

    /// Slack Signing Secret for webhook signature verification.
    pub slack_signing_secret: Option<String>,

    /// Turso database URL (local file or cloud).
    /// Default: ~/.local/share/temperpaw/paw.db
    pub turso_url: Option<String>,

    /// Turso auth token (for Turso Cloud).
    pub turso_auth_token: Option<String>,

    /// Postgres connection URL for Railway-managed Postgres.
    pub database_url: Option<String>,

    /// Runtime event store backend. Defaults to Turso for local compatibility.
    pub event_store_backend: StorageBackend,

    /// Platform metadata backend. Defaults to the event store backend.
    pub platform_store_backend: StorageBackend,

    /// Query projection backend. Defaults to the event store backend.
    pub query_projection_store_backend: StorageBackend,

    /// Anthropic API key (sk-ant-api03-... from console.anthropic.com).
    pub anthropic_api_key: Option<String>,

    /// OpenRouter API key.
    pub openrouter_api_key: Option<String>,

    /// Optional OpenRouter OpenAI-compatible endpoint override.
    pub openrouter_api_url: Option<String>,

    /// Hugging Face API token for Inference Providers.
    pub huggingface_api_key: Option<String>,

    /// Optional Hugging Face OpenAI-compatible endpoint override.
    pub huggingface_api_url: Option<String>,

    /// Fireworks AI API key.
    pub fireworks_api_key: Option<String>,

    /// Optional Fireworks OpenAI-compatible endpoint override.
    pub fireworks_api_url: Option<String>,

    /// Sakana Fugu beta API key.
    pub sakana_fugu_api_key: Option<String>,

    /// Sakana Fugu beta OpenAI-compatible endpoint.
    pub sakana_fugu_api_url: Option<String>,

    /// Arbitrary OpenAI-compatible API key.
    pub openai_compatible_api_key: Option<String>,

    /// Arbitrary OpenAI-compatible chat completions endpoint.
    pub openai_compatible_api_url: Option<String>,

    /// JSON object of extra headers for arbitrary OpenAI-compatible endpoints.
    pub openai_compatible_headers_json: Option<String>,

    /// Local OpenAI-compatible chat completions endpoint (Ollama by default).
    pub local_openai_api_url: Option<String>,

    /// OpenAI API key for standard API usage (sk-... from platform.openai.com/api-keys).
    pub openai_api_key: Option<String>,

    /// Legacy OpenAI Codex OAuth token fallback. Prefer dashboard device-code login.
    pub openai_codex_token: Option<String>,

    /// LLM provider name, e.g. "anthropic", "openai_codex", "openrouter", or
    /// any supported OpenAI-compatible provider.
    /// Set during interactive setup or via the Temper secrets world.
    pub llm_provider: Option<String>,

    /// LLM model identifier selected during setup.
    pub llm_model: Option<String>,

    /// Tensorlake API key for remote sandbox provisioning.
    pub tensorlake_api_key: Option<String>,

    /// Sandbox provider name: "tensorlake" or "modal".
    pub sandbox_provider: Option<String>,

    /// Modal token ID (starts with ak-…).
    pub modal_token_id: Option<String>,

    /// Modal token secret (starts with as-…).
    pub modal_token_secret: Option<String>,

    /// Modal bridge base URL used by the sandbox helper.
    pub modal_bridge_url: Option<String>,

    /// GitHub token for repo cloning and PR flows.
    pub github_token: Option<String>,

    /// Genesis (temper-git) token for pushing app bundles to the Genesis
    /// registry. Genesis requires auth on push; supplied as the Basic-auth
    /// username on git pushes to the Genesis host.
    pub genesis_token: Option<String>,

    /// Datadog API key for monitor and events APIs.
    pub dd_api_key: Option<String>,

    /// Datadog application key for monitor and events APIs.
    pub dd_app_key: Option<String>,

    /// Datadog site suffix, for example `datadoghq.com`.
    pub dd_site: String,

    /// Bearer token for Temper API authentication.
    pub temper_api_key: Option<String>,

    /// 32-byte base64 key for secrets vault encryption.
    /// If not set, an ephemeral key is generated (secrets lost on restart).
    pub vault_key: Option<String>,

    /// Fly.io API token for Sprites provisioning.
    pub fly_api_token: Option<String>,

    /// Railway API token for deployment integrations.
    pub railway_token: Option<String>,

    /// Railway project ID (captured during deploy for dashboard → Railway API).
    pub railway_project_id: Option<String>,

    /// Railway environment ID (captured during deploy).
    pub railway_environment_id: Option<String>,

    /// Railway otel-collector service ID (captured during deploy).
    pub railway_otel_service_id: Option<String>,

    /// Railway Datadog runtime Agent service ID (captured during deploy).
    pub railway_datadog_runtime_agent_service_id: Option<String>,

    /// Railway main temperpaw service ID (for redeploy from dashboard).
    pub railway_service_id: Option<String>,

    /// Vercel API token for deployment integrations.
    pub vercel_token: Option<String>,

    /// Exa API key for web search.
    pub exa_api_key: Option<String>,

    /// Enable OpenTelemetry export to Datadog Agent via OTLP.
    pub otel_enabled: bool,

    /// OTLP gRPC endpoint (Datadog Agent).
    pub otel_endpoint: String,

    /// HTTP port for the OData API + webhook listener.
    pub port: u16,

    /// Public base URL used for externally reachable webhook URLs.
    pub public_base_url: Option<String>,

    /// Optional ngrok authtoken used for automatic local Discord interaction tunnels.
    pub ngrok_authtoken: Option<String>,

    /// ngrok binary to use when automatically creating a local Discord tunnel.
    pub ngrok_bin: String,

    /// Build version string (set at compile time via BUILD_VERSION env var).
    pub build_version: String,

    /// Build git SHA (set at compile time via BUILD_SHA env var).
    pub build_sha: String,

    /// Default tenant ID.
    pub tenant: String,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();

        let anthropic_api_key = optional_env("ANTHROPIC_API_KEY");
        let openrouter_api_key = optional_env("OPENROUTER_API_KEY");
        let openrouter_api_url = optional_env("OPENROUTER_API_URL");
        let huggingface_api_key =
            optional_env("HUGGINGFACE_API_KEY").or_else(|| optional_env("HF_TOKEN"));
        let huggingface_api_url = optional_env("HUGGINGFACE_API_URL");
        let fireworks_api_key = optional_env("FIREWORKS_API_KEY");
        let fireworks_api_url = optional_env("FIREWORKS_API_URL");
        let sakana_fugu_api_key = optional_env("SAKANA_FUGU_API_KEY");
        let sakana_fugu_api_url = optional_env("SAKANA_FUGU_API_URL");
        let openai_compatible_api_key = optional_env("OPENAI_COMPATIBLE_API_KEY");
        let openai_compatible_api_url = optional_env("OPENAI_COMPATIBLE_API_URL");
        let openai_compatible_headers_json = optional_env("OPENAI_COMPATIBLE_HEADERS_JSON");
        let local_openai_api_url = optional_env("LOCAL_OPENAI_API_URL");
        let openai_api_key = optional_env("OPENAI_API_KEY");
        let openai_codex_token = optional_env("OPENAI_CODEX_TOKEN");
        let llm_provider = optional_env("LLM_PROVIDER");
        let llm_model = optional_env("LLM_MODEL");
        let event_store_backend = parse_storage_backend("TEMPER_EVENT_STORE", None)?;
        let platform_store_backend =
            parse_storage_backend("TEMPER_PLATFORM_STORE", Some(event_store_backend))?;
        let query_projection_store_backend =
            parse_storage_backend("TEMPER_QUERY_PROJECTION_STORE", Some(event_store_backend))?;

        Ok(Self {
            discord_bot_token: optional_env("DISCORD_BOT_TOKEN"),
            discord_public_key: optional_env("DISCORD_PUBLIC_KEY"),
            discord_guild_id: optional_env("DISCORD_GUILD_ID"),
            discord_feed_channel_id: optional_env("DISCORD_FEED_CHANNEL_ID"),
            discord_forum_channel_id: optional_env("DISCORD_FORUM_CHANNEL_ID"),
            slack_app_token: optional_env("SLACK_APP_TOKEN"),
            slack_bot_token: optional_env("SLACK_BOT_TOKEN"),
            slack_signing_secret: optional_env("SLACK_SIGNING_SECRET"),
            turso_url: optional_env("TURSO_URL"),
            turso_auth_token: optional_env("TURSO_AUTH_TOKEN"),
            database_url: optional_env("DATABASE_URL"),
            event_store_backend,
            platform_store_backend,
            query_projection_store_backend,
            anthropic_api_key,
            openrouter_api_key,
            openrouter_api_url,
            huggingface_api_key,
            huggingface_api_url,
            fireworks_api_key,
            fireworks_api_url,
            sakana_fugu_api_key,
            sakana_fugu_api_url,
            openai_compatible_api_key,
            openai_compatible_api_url,
            openai_compatible_headers_json,
            local_openai_api_url,
            openai_api_key,
            openai_codex_token,
            llm_provider,
            llm_model,
            tensorlake_api_key: optional_env("TL_API_KEY"),
            sandbox_provider: optional_env("SANDBOX_PROVIDER"),
            modal_token_id: optional_env("MODAL_TOKEN_ID"),
            modal_token_secret: optional_env("MODAL_TOKEN_SECRET"),
            modal_bridge_url: optional_env("MODAL_BRIDGE_URL"),
            github_token: optional_env("GITHUB_TOKEN"),
            genesis_token: optional_env("GENESIS_TOKEN"),
            dd_api_key: optional_env("DD_API_KEY"),
            dd_app_key: optional_env("DD_APP_KEY"),
            dd_site: std::env::var("DD_SITE").unwrap_or_else(|_| "datadoghq.com".to_string()),
            temper_api_key: optional_env("TEMPER_API_KEY"),
            vault_key: optional_env("TEMPER_VAULT_KEY"),
            fly_api_token: optional_env("FLY_API_TOKEN"),
            railway_token: optional_env("RAILWAY_TOKEN"),
            railway_project_id: optional_env("RAILWAY_PROJECT_ID"),
            railway_environment_id: optional_env("RAILWAY_ENVIRONMENT_ID"),
            railway_otel_service_id: optional_env("RAILWAY_OTEL_SERVICE_ID"),
            railway_datadog_runtime_agent_service_id: optional_env(
                "RAILWAY_DATADOG_RUNTIME_AGENT_SERVICE_ID",
            ),
            railway_service_id: optional_env("RAILWAY_SERVICE_ID"),
            vercel_token: optional_env("VERCEL_TOKEN"),
            exa_api_key: optional_env("EXA_API_KEY"),
            otel_enabled: std::env::var("OTEL_ENABLED")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            otel_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4318".to_string()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3467),
            public_base_url: optional_env("PUBLIC_BASE_URL").or_else(|| {
                optional_env("RAILWAY_PUBLIC_DOMAIN").map(|domain| format!("https://{domain}"))
            }),
            ngrok_authtoken: optional_env("NGROK_AUTHTOKEN"),
            ngrok_bin: std::env::var("NGROK_BIN").unwrap_or_else(|_| "ngrok".to_string()),
            build_version: std::env::var("BUILD_VERSION").unwrap_or_else(|_| "dev".to_string()),
            build_sha: std::env::var("BUILD_SHA").unwrap_or_else(|_| "unknown".to_string()),
            tenant: std::env::var("PAW_TENANT").unwrap_or_else(|_| "default".to_string()),
        })
    }
}

fn parse_storage_backend(
    name: &str,
    default: Option<StorageBackend>,
) -> anyhow::Result<StorageBackend> {
    match optional_env(name) {
        Some(value) => StorageBackend::from_env_value(name, &value),
        None => default
            .or(Some(StorageBackend::Turso))
            .with_context(|| format!("{name} has no default")),
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{Config, StorageBackend};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_storage_env(test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let names = [
            "TEMPER_EVENT_STORE",
            "TEMPER_PLATFORM_STORE",
            "TEMPER_QUERY_PROJECTION_STORE",
            "DATABASE_URL",
            "TURSO_URL",
            "TURSO_AUTH_TOKEN",
        ];
        let previous: Vec<_> = names
            .iter()
            .map(|name| (*name, std::env::var(name).ok()))
            .collect();
        for name in names {
            unsafe {
                std::env::remove_var(name);
            }
        }

        test();

        for (name, value) in previous {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn from_env_selects_postgres_storage_from_temper_event_store() {
        with_clean_storage_env(|| {
            unsafe {
                std::env::set_var("TEMPER_EVENT_STORE", "postgres");
                std::env::set_var("DATABASE_URL", "postgres://temperpaw.example/db");
            }

            let config = Config::from_env().expect("config");

            assert_eq!(config.event_store_backend, StorageBackend::Postgres);
            assert_eq!(
                config.database_url.as_deref(),
                Some("postgres://temperpaw.example/db")
            );
            assert_eq!(
                config.platform_store_backend,
                StorageBackend::Postgres,
                "platform storage should follow the event store backend by default"
            );
            assert_eq!(
                config.query_projection_store_backend,
                StorageBackend::Postgres,
                "query projection storage should follow the event store backend by default"
            );
        });
    }

    #[test]
    fn from_env_defaults_to_turso_for_local_compatibility() {
        with_clean_storage_env(|| {
            let config = Config::from_env().expect("config");

            assert_eq!(config.event_store_backend, StorageBackend::Turso);
            assert_eq!(config.platform_store_backend, StorageBackend::Turso);
            assert_eq!(config.query_projection_store_backend, StorageBackend::Turso);
            assert!(config.database_url.is_none());
        });
    }
}
