//! Runtime transport lifecycle management.
//!
//! Provides start/stop/status control over Discord and Slack transports
//! without requiring a process restart. Stores a `CancellationToken` per
//! transport so they can be gracefully shut down via the setup API.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const DISCORD_WEBHOOK_PORT: u16 = 3488;
pub const SLACK_WEBHOOK_PORT: u16 = 3489;

/// Manages the lifecycle of Discord and Slack transports at runtime.
pub struct TransportManager {
    discord: Arc<RwLock<TransportHandle>>,
    slack: Arc<RwLock<TransportHandle>>,
    ngrok: Arc<RwLock<Option<NgrokTunnel>>>,
    tenant: String,
    port: u16,
    credentials: temper_server::internal_invocation::InternalInvocationCredentialStore,
    public_base_url: Arc<RwLock<Option<String>>>,
    ngrok_bin: String,
    ngrok_authtoken: Option<String>,
}

struct TransportHandle {
    status: TransportStatus,
    cancel: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

struct NgrokTunnel {
    #[allow(dead_code)]
    child: Child,
    public_base_url: String,
}

#[derive(Debug, Deserialize)]
struct NgrokApiResponse {
    #[serde(default)]
    tunnels: Vec<NgrokApiTunnel>,
}

#[derive(Debug, Deserialize)]
struct NgrokApiTunnel {
    public_url: String,
    #[serde(default)]
    config: NgrokApiTunnelConfig,
}

#[derive(Debug, Default, Deserialize)]
struct NgrokApiTunnelConfig {
    #[serde(default)]
    addr: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TransportStatus {
    Disconnected,
    Connecting,
    Connected {
        #[serde(skip_serializing_if = "Option::is_none")]
        guild_id: Option<String>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AllTransportStatus {
    pub discord: TransportStatus,
    pub slack: TransportStatus,
}

/// Discord connection parameters.
pub struct DiscordConnectParams {
    pub bot_token: String,
    pub public_key: Option<String>,
    pub guild_id: Option<String>,
    pub feed_channel_id: Option<String>,
    pub forum_channel_id: Option<String>,
}

/// Slack connection parameters.
pub struct SlackConnectParams {
    pub app_token: String,
    pub bot_token: String,
    pub signing_secret: String,
}

impl TransportManager {
    pub fn new(
        tenant: String,
        port: u16,
        credentials: temper_server::internal_invocation::InternalInvocationCredentialStore,
        public_base_url: Option<String>,
        ngrok_bin: String,
        ngrok_authtoken: Option<String>,
    ) -> Self {
        Self {
            discord: Arc::new(RwLock::new(TransportHandle {
                status: TransportStatus::Disconnected,
                cancel: None,
                task: None,
            })),
            slack: Arc::new(RwLock::new(TransportHandle {
                status: TransportStatus::Disconnected,
                cancel: None,
                task: None,
            })),
            ngrok: Arc::new(RwLock::new(None)),
            tenant,
            port,
            credentials,
            public_base_url: Arc::new(RwLock::new(
                public_base_url.map(|url| url.trim_end_matches('/').to_string()),
            )),
            ngrok_bin,
            ngrok_authtoken,
        }
    }

    pub fn ngrok_available(ngrok_bin: &str) -> bool {
        std::process::Command::new(ngrok_bin)
            .arg("version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    async fn current_public_base_url(&self) -> Option<String> {
        self.public_base_url.read().await.clone()
    }

    pub async fn discord_interaction_public_url(&self) -> Option<String> {
        self.current_public_base_url()
            .await
            .map(|base_url| format!("{}/discord/interaction", base_url.trim_end_matches('/')))
    }

    async fn ensure_discord_public_base_url(&self) -> anyhow::Result<String> {
        if let Some(public_base_url) = self.current_public_base_url().await {
            return Ok(public_base_url);
        }

        if let Some(public_base_url) = self.lookup_ngrok_public_base_url().await? {
            tracing::info!(%public_base_url, "Reusing existing ngrok tunnel for Discord interactions");
            *self.public_base_url.write().await = Some(public_base_url.clone());
            return Ok(public_base_url);
        }

        if !Self::ngrok_available(&self.ngrok_bin) {
            bail!(
                "Discord requires a public interactions URL. Set PUBLIC_BASE_URL or install/configure {}.",
                self.ngrok_bin
            );
        }

        let mut command = Command::new(&self.ngrok_bin);
        command
            .arg("http")
            .arg(self.port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        if let Some(authtoken) = self.ngrok_authtoken.as_ref() {
            command.env("NGROK_AUTHTOKEN", authtoken);
        }

        let child = command.spawn().with_context(|| {
            format!(
                "Failed to launch {} for Discord interactions",
                self.ngrok_bin
            )
        })?;
        {
            let mut ngrok = self.ngrok.write().await;
            *ngrok = Some(NgrokTunnel {
                child,
                public_base_url: String::new(),
            });
        }
        tracing::info!(
            ngrok_bin = %self.ngrok_bin,
            port = self.port,
            "Started ngrok automatically for local Discord interactions"
        );

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(public_base_url) = self.lookup_ngrok_public_base_url().await? {
                {
                    let mut ngrok = self.ngrok.write().await;
                    if let Some(tunnel) = ngrok.as_mut() {
                        tunnel.public_base_url = public_base_url.clone();
                    }
                }
                *self.public_base_url.write().await = Some(public_base_url.clone());
                tracing::info!(%public_base_url, "Discord interactions available through ngrok");
                return Ok(public_base_url);
            }
        }

        bail!(
            "Started {} automatically, but could not discover a public URL. Check ngrok and set PUBLIC_BASE_URL manually if needed.",
            self.ngrok_bin
        );
    }

    async fn lookup_ngrok_public_base_url(&self) -> anyhow::Result<Option<String>> {
        let response = match reqwest::get("http://127.0.0.1:4040/api/tunnels").await {
            Ok(response) => response,
            Err(_) => return Ok(None),
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        let payload: NgrokApiResponse = response
            .json()
            .await
            .context("Failed to parse ngrok tunnels response")?;
        let port_suffix = format!(":{}", self.port);
        let fallback_https = payload
            .tunnels
            .iter()
            .find(|tunnel| tunnel.public_url.starts_with("https://"))
            .map(|tunnel| tunnel.public_url.trim_end_matches('/').to_string());

        Ok(payload
            .tunnels
            .into_iter()
            .find(|tunnel| {
                tunnel.public_url.starts_with("https://")
                    && (tunnel.config.addr.contains(&port_suffix)
                        || tunnel.config.addr.ends_with(&self.port.to_string()))
            })
            .map(|tunnel| tunnel.public_url.trim_end_matches('/').to_string())
            .or(fallback_https))
    }

    /// Start the Discord transport. If already connected, disconnects first.
    pub async fn connect_discord(&self, params: DiscordConnectParams) -> anyhow::Result<String> {
        let public_key =
            crate::discord_app::resolve_verify_key(&params.bot_token, params.public_key.as_deref())
                .await?;

        let public_base_url = self.ensure_discord_public_base_url().await?;
        let interaction_url = format!(
            "{}/discord/interaction",
            public_base_url.trim_end_matches('/')
        );

        // Disconnect existing if any
        self.disconnect_discord().await;

        let cancel = CancellationToken::new();
        {
            let mut handle = self.discord.write().await;
            handle.status = TransportStatus::Connecting;
            handle.cancel = Some(cancel.clone());
        }

        let discord_handle = self.discord.clone();
        let tenant = self.tenant.clone();
        let port = self.port;
        let credentials = self.credentials.clone();
        let guild_id_for_status = params.guild_id.clone();
        let (ready_tx, mut ready_rx) = watch::channel(false);
        let (startup_tx, startup_rx) = oneshot::channel();

        let task = tokio::spawn(async move {
            use paw_transport::discord::types::intents;
            use paw_transport::discord::{DiscordConfig, DiscordTransport};

            tracing::info!("Discord transport: connecting (tenant={tenant})...");

            let api = crate::auth::local_api_client(&credentials, &tenant, port);
            let config = DiscordConfig {
                bot_token: params.bot_token,
                public_key,
                intents: intents::DEFAULT,
                webhook_port: DISCORD_WEBHOOK_PORT,
                guild_id: params.guild_id,
                feed_channel_id: params.feed_channel_id,
                forum_channel_id: params.forum_channel_id,
            };
            let transport = DiscordTransport::new(config, api).with_ready_signal(ready_tx);
            let run = transport.run();
            tokio::pin!(run);
            let mut startup_tx = Some(startup_tx);
            let mut ready_observed = false;

            loop {
                tokio::select! {
                    result = &mut run => {
                        let mut handle = discord_handle.write().await;
                        match result {
                            Ok(()) => {
                                handle.status = TransportStatus::Disconnected;
                                if let Some(startup_tx) = startup_tx.take() {
                                    let _ = startup_tx.send(Err(anyhow!("Discord transport exited before becoming ready")));
                                }
                            }
                            Err(e) => {
                                tracing::error!("Discord transport error: {e}");
                                handle.status = TransportStatus::Error {
                                    message: e.to_string(),
                                };
                                if let Some(startup_tx) = startup_tx.take() {
                                    let _ = startup_tx.send(Err(anyhow!(e)));
                                }
                            }
                        }
                        handle.cancel = None;
                        break;
                    }
                    changed = ready_rx.changed(), if !ready_observed => {
                        match changed {
                            Ok(()) if *ready_rx.borrow() => {
                                ready_observed = true;
                                let mut handle = discord_handle.write().await;
                                handle.status = TransportStatus::Connected {
                                    guild_id: guild_id_for_status.clone(),
                                };
                                if let Some(startup_tx) = startup_tx.take() {
                                    let _ = startup_tx.send(Ok(()));
                                }
                            }
                            Ok(()) => {}
                            Err(_) => {}
                        }
                    }
                    _ = cancel.cancelled() => {
                        tracing::info!("Discord transport: shutting down (cancelled)");
                        let mut handle = discord_handle.write().await;
                        handle.status = TransportStatus::Disconnected;
                        handle.cancel = None;
                        if let Some(startup_tx) = startup_tx.take() {
                            let _ = startup_tx.send(Err(anyhow!("Discord transport startup was cancelled")));
                        }
                        break;
                    }
                }
            }
        });

        {
            let mut handle = self.discord.write().await;
            handle.task = Some(task);
        }

        match tokio::time::timeout(Duration::from_secs(30), startup_rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => return Err(error),
            Ok(Err(_)) => bail!("Discord transport startup task ended unexpectedly"),
            Err(_) => {
                self.disconnect_discord().await;
                bail!("Timed out waiting for Discord to reach READY");
            }
        }

        Ok(interaction_url)
    }

    /// Stop the Discord transport gracefully.
    pub async fn disconnect_discord(&self) {
        self.stop_transport(&self.discord, "discord").await;
    }

    /// Start the Slack transport. If already connected, disconnects first.
    pub async fn connect_slack(&self, params: SlackConnectParams) {
        self.disconnect_slack().await;

        let cancel = CancellationToken::new();
        {
            let mut handle = self.slack.write().await;
            handle.status = TransportStatus::Connecting;
            handle.cancel = Some(cancel.clone());
        }

        let slack_handle = self.slack.clone();
        let tenant = self.tenant.clone();
        let port = self.port;
        let credentials = self.credentials.clone();

        let task = tokio::spawn(async move {
            use paw_transport::slack::{SlackConfig, SlackTransport};

            tracing::info!("Slack transport: connecting (tenant={tenant})...");

            let api = crate::auth::local_api_client(&credentials, &tenant, port);
            let config = SlackConfig {
                app_token: params.app_token,
                bot_token: params.bot_token,
                signing_secret: params.signing_secret,
                webhook_port: SLACK_WEBHOOK_PORT,
            };

            {
                let mut handle = slack_handle.write().await;
                handle.status = TransportStatus::Connected { guild_id: None };
            }

            let transport = SlackTransport::new(config, api);
            tokio::select! {
                result = transport.run() => {
                    let mut handle = slack_handle.write().await;
                    match result {
                        Ok(()) => {
                            handle.status = TransportStatus::Disconnected;
                        }
                        Err(e) => {
                            tracing::error!("Slack transport error: {e}");
                            handle.status = TransportStatus::Error {
                                message: e.to_string(),
                            };
                        }
                    }
                    handle.cancel = None;
                }
                _ = cancel.cancelled() => {
                    tracing::info!("Slack transport: shutting down (cancelled)");
                    let mut handle = slack_handle.write().await;
                    handle.status = TransportStatus::Disconnected;
                    handle.cancel = None;
                }
            }
        });

        {
            let mut handle = self.slack.write().await;
            handle.task = Some(task);
        }
    }

    /// Stop the Slack transport gracefully.
    pub async fn disconnect_slack(&self) {
        self.stop_transport(&self.slack, "slack").await;
    }

    /// Get the current status of all transports.
    pub async fn status(&self) -> AllTransportStatus {
        AllTransportStatus {
            discord: self.discord.read().await.status.clone(),
            slack: self.slack.read().await.status.clone(),
        }
    }

    pub fn discord_interaction_proxy_url(&self) -> String {
        format!("http://127.0.0.1:{DISCORD_WEBHOOK_PORT}/interaction")
    }

    async fn stop_transport(&self, handle_lock: &Arc<RwLock<TransportHandle>>, label: &str) {
        let (cancel, task) = {
            let mut handle = handle_lock.write().await;
            handle.status = TransportStatus::Disconnected;
            (handle.cancel.take(), handle.task.take())
        };

        if let Some(cancel) = cancel {
            cancel.cancel();
        }

        if let Some(task) = task {
            match task.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    tracing::warn!(transport = label, %error, "transport task join failed");
                }
            }
        }
    }
}

pub fn ngrok_available(ngrok_bin: &str) -> bool {
    TransportManager::ngrok_available(ngrok_bin)
}
