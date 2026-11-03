//! Discord transport — wires Discord Gateway to Paw Channel entities.
//!
//! On startup: bootstraps the Channel entity used for Discord delivery.
//! On MESSAGE_CREATE: dispatches Channel.ReceiveMessage via OData API.
//! On Channel.SendReply events: delivers reply via Discord REST API.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{RwLock, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::Instrument;

use super::gateway::*;
use super::types::*;
use crate::{
    PawApiClient, apply_current_trace_context, approval_body_for_scope, approval_scope_from_action,
    fetch_pending_decision,
};

/// Configuration for the Discord transport.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Discord bot token.
    pub bot_token: String,
    /// Gateway intents bitmask.
    pub intents: u32,
    /// Port for the webhook listener (receives replies from send_reply WASM).
    /// Defaults to 0 (auto-assign).
    pub webhook_port: u16,
    /// Discord application public key (hex) for interaction signature verification.
    pub public_key: String,
    /// Discord guild (server) ID for observability channels.
    pub guild_id: Option<String>,
    /// Discord text channel ID for the #feed stream.
    pub feed_channel_id: Option<String>,
    /// Discord forum channel ID for per-agent threads.
    pub forum_channel_id: Option<String>,
}

/// Discord channel transport.
///
/// Connects to Discord Gateway, dispatches messages to Paw Channel entities
/// via the OData API, and delivers replies via Discord REST API.
pub struct DiscordTransport {
    config: DiscordConfig,
    api: PawApiClient,
    http: reqwest::Client,
    gateway: GatewayState,
    /// Channel entity ID in Paw (populated on startup).
    channel_entity_id: Arc<RwLock<Option<String>>>,
    /// Local reply webhook URL used to reconnect Channel entities after recovery.
    reply_webhook_url: Arc<RwLock<Option<String>>>,
    /// Maps user/thread IDs to Discord DM channel IDs for reply routing and catch-up.
    dm_channels: Arc<RwLock<BTreeMap<String, String>>>,
    /// Last processed Discord message snowflake ID (for catch-up on reconnect).
    last_message_cursor: Arc<RwLock<String>>,
    /// Cancel senders for active typing indicator loops, keyed by thread_id (user ID for DMs).
    /// When a reply is sent, the corresponding cancel sender is triggered to stop the typing loop.
    typing_cancels: Arc<RwLock<BTreeMap<String, tokio::sync::watch::Sender<bool>>>>,
    /// Optional notifier used by the runtime transport manager to detect the first READY event.
    ready_signal: Option<watch::Sender<bool>>,
}

struct WebhookListenerGuard {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl WebhookListenerGuard {
    fn new(port: u16, shutdown: tokio::sync::oneshot::Sender<()>, task: JoinHandle<()>) -> Self {
        Self {
            port,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for WebhookListenerGuard {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn embed_text_len(embed: &Embed) -> usize {
    let title_len = embed.title.as_ref().map(|value| value.len()).unwrap_or(0);
    let description_len = embed
        .description
        .as_ref()
        .map(|value| value.len())
        .unwrap_or(0);
    let field_len = embed
        .fields
        .as_ref()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field.name.len() + field.value.len())
                .sum::<usize>()
        })
        .unwrap_or(0);
    let footer_len = embed
        .footer
        .as_ref()
        .map(|value| value.text.len())
        .unwrap_or(0);
    title_len + description_len + field_len + footer_len
}

fn embeds_exceed_limits(embeds: &[Embed]) -> bool {
    let total_chars = embeds.iter().map(embed_text_len).sum::<usize>();
    total_chars > 6000
        || embeds.iter().any(|embed| {
            embed
                .title
                .as_ref()
                .map(|value| value.len() > 256)
                .unwrap_or(false)
                || embed
                    .description
                    .as_ref()
                    .map(|value| value.len() > 4096)
                    .unwrap_or(false)
                || embed
                    .footer
                    .as_ref()
                    .map(|value| value.text.len() > 2048)
                    .unwrap_or(false)
                || embed
                    .fields
                    .as_ref()
                    .map(|fields| {
                        fields.len() > 25
                            || fields
                                .iter()
                                .any(|field| field.name.len() > 256 || field.value.len() > 1024)
                    })
                    .unwrap_or(false)
        })
}

fn flatten_embeds_for_plain_text(embeds: &[Embed]) -> String {
    let mut blocks = Vec::new();
    for embed in embeds {
        let mut parts = Vec::new();
        if let Some(title) = embed.title.as_ref().filter(|value| !value.is_empty()) {
            parts.push(format!("**{title}**"));
        }
        if let Some(description) = embed.description.as_ref().filter(|value| !value.is_empty()) {
            parts.push(description.clone());
        }
        if let Some(fields) = embed.fields.as_ref() {
            for field in fields {
                parts.push(format!("**{}**\n{}", field.name, field.value));
            }
        }
        if let Some(footer) = embed.footer.as_ref().filter(|value| !value.text.is_empty()) {
            parts.push(format!("_{text}_", text = footer.text));
        }
        if !parts.is_empty() {
            blocks.push(parts.join("\n\n"));
        }
    }
    blocks.join("\n\n")
}

fn build_rich_fallback_text(content: &str, embeds: &[Embed]) -> String {
    let mut parts = Vec::new();
    if !content.is_empty() {
        parts.push(content.to_string());
    }
    let embed_text = flatten_embeds_for_plain_text(embeds);
    if !embed_text.is_empty() {
        parts.push(embed_text);
    }
    parts.join("\n\n")
}

#[derive(Debug, Clone, Deserialize)]
struct ReplyAttachment {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    file_id: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    path: String,
}

fn parse_reply_attachments(value: &serde_json::Value) -> Result<Vec<ReplyAttachment>, String> {
    let Some(raw) = value
        .get("reply_attachments_json")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };

    serde_json::from_str::<Vec<ReplyAttachment>>(raw)
        .map_err(|err| format!("parse reply_attachments_json: {err}"))
}

async fn deliver_reply_attachments(
    api: &PawApiClient,
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    attachments: &[ReplyAttachment],
) -> Result<bool, String> {
    if attachments.is_empty() {
        return Ok(false);
    }

    let mut files = Vec::new();
    for attachment in attachments {
        if attachment.kind != "pawfs_file" {
            continue;
        }
        files.push(download_pawfs_attachment(api, attachment).await?);
    }

    if files.is_empty() {
        return Ok(false);
    }

    send_discord_message_with_files(http, bot_token, channel_id, content, &files)
        .await
        .map_err(|err| format!("discord file reply delivery failed: {err}"))?;
    Ok(true)
}

async fn download_pawfs_attachment(
    api: &PawApiClient,
    attachment: &ReplyAttachment,
) -> Result<DiscordFileUpload, String> {
    let file_id = attachment.file_id.trim();
    if file_id.is_empty() {
        return Err("reply attachment missing file_id".to_string());
    }
    let url = format!(
        "{}/tdata/Files('{}')/$value",
        api.config().base_url,
        escape_odata_key(file_id)
    );
    let bytes = api.raw_get_bytes(&url).await?;
    Ok(DiscordFileUpload {
        filename: reply_attachment_filename(attachment),
        content_type: if attachment.mime_type.trim().is_empty() {
            "image/png".to_string()
        } else {
            attachment.mime_type.trim().to_string()
        },
        bytes,
    })
}

fn reply_attachment_filename(attachment: &ReplyAttachment) -> String {
    attachment
        .filename
        .trim()
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            attachment
                .path
                .trim()
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("image.png")
        .to_string()
}

fn escape_odata_key(value: &str) -> String {
    value.replace('\'', "''")
}

fn is_direct_message_thread_id(thread_id: &str) -> bool {
    !thread_id.is_empty() && !thread_id.contains(':')
}

fn entity_field_str<'a>(entity: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    let fields = entity.get("fields").unwrap_or(entity);
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(|value| value.as_str()))
        .filter(|value| !value.is_empty())
}

fn known_dm_thread_from_channel_entity(channel: &serde_json::Value) -> Option<String> {
    let thread_id = entity_field_str(channel, &["thread_id", "ThreadId"])
        .or_else(|| entity_field_str(channel, &["author_id", "AuthorId"]))?;
    is_direct_message_thread_id(thread_id).then(|| thread_id.to_string())
}

fn catch_up_dm_sources(
    rest_dm_channels: &[serde_json::Value],
    seeded_dm_channels: &BTreeMap<String, String>,
    bot_id: &str,
) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();

    for dm in rest_dm_channels {
        let Some(channel_id) = dm
            .get("id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let Some(recipients) = dm.get("recipients").and_then(|value| value.as_array()) else {
            continue;
        };

        for recipient in recipients {
            let Some(recipient_id) = recipient
                .get("id")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty() && *value != bot_id)
            else {
                continue;
            };
            sources.insert(recipient_id.to_string(), channel_id.to_string());
        }
    }

    for (thread_id, channel_id) in seeded_dm_channels {
        if is_direct_message_thread_id(thread_id) && !channel_id.is_empty() {
            sources
                .entry(thread_id.clone())
                .or_insert_with(|| channel_id.clone());
        }
    }

    sources
}

async fn resolve_dm_channel_id(
    http: &reqwest::Client,
    bot_token: &str,
    dm_channels: &Arc<RwLock<BTreeMap<String, String>>>,
    thread_id: &str,
) -> Result<String, DiscordApiError> {
    resolve_dm_channel_id_at(http, bot_token, dm_channels, thread_id, DISCORD_API_BASE).await
}

async fn resolve_dm_channel_id_at(
    http: &reqwest::Client,
    bot_token: &str,
    dm_channels: &Arc<RwLock<BTreeMap<String, String>>>,
    thread_id: &str,
    discord_api_base: &str,
) -> Result<String, DiscordApiError> {
    if thread_id.is_empty() {
        return Err(DiscordApiError::RequestFailed(
            "discord reply webhook has no thread_id".to_string(),
        ));
    }

    if let Some(channel_id) = dm_channels.read().await.get(thread_id).cloned() {
        return Ok(channel_id);
    }

    tracing::warn!(
        thread_id,
        "discord reply webhook missing DM channel cache; reopening DM channel"
    );
    let channel_id = open_dm_channel_at(http, bot_token, discord_api_base, thread_id).await?;
    dm_channels
        .write()
        .await
        .insert(thread_id.to_string(), channel_id.clone());
    Ok(channel_id)
}

// ── Attachment helpers ───────────────────────────────────────────────

/// Maximum size (bytes) of a single text attachment to inline.
const MAX_ATTACHMENT_SIZE: u64 = 100_000;

/// Check whether a Discord attachment is a text-type file worth inlining.
///
/// Uses `content_type` when available, otherwise falls back to file extension.
fn is_text_attachment(att: &DiscordAttachment) -> bool {
    if let Some(ct) = &att.content_type {
        let ct_lower = ct.to_lowercase();
        if ct_lower.starts_with("text/") {
            return true;
        }
        if matches!(
            ct_lower.as_str(),
            "application/json"
                | "application/xml"
                | "application/toml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/javascript"
                | "application/typescript"
                | "application/x-sh"
        ) {
            return true;
        }
        return false;
    }
    // Fallback: check file extension.
    let name = att.filename.to_lowercase();
    let text_exts = [
        ".md", ".txt", ".rs", ".py", ".ts", ".js", ".json", ".toml", ".yaml", ".yml", ".csv",
        ".html", ".css", ".xml", ".sh", ".bash", ".go", ".java", ".c", ".cpp", ".h", ".rb", ".php",
        ".sql", ".log", ".cfg", ".ini", ".conf", ".env", ".tsx", ".jsx", ".svelte", ".vue", ".tf",
        ".hcl", ".prisma", ".graphql", ".proto",
    ];
    text_exts.iter().any(|ext| name.ends_with(ext))
}

/// Download text-type attachments from Discord CDN.
///
/// Skips non-text and oversized files. Returns `(filename, content)` pairs.
/// Fault-tolerant: logs warnings on download failure, never fails the message.
async fn fetch_text_attachments(
    http: &reqwest::Client,
    attachments: &[DiscordAttachment],
) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for att in attachments {
        if !is_text_attachment(att) {
            continue;
        }
        if att.size > MAX_ATTACHMENT_SIZE {
            tracing::warn!(
                attachment_filename = %att.filename,
                attachment_size = att.size,
                max_attachment_size = MAX_ATTACHMENT_SIZE,
                "discord text attachment skipped because it exceeds the inline size limit"
            );
            continue;
        }
        let mut failure = None;
        for url in attachment_download_urls(att) {
            match http.get(url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.text().await {
                    Ok(body) => {
                        results.push((att.filename.clone(), body));
                        failure = None;
                        break;
                    }
                    Err(e) => {
                        failure = Some(format!("body could not be read: {e}"));
                    }
                },
                Ok(resp) => {
                    failure = Some(format!("download returned {}", resp.status()));
                }
                Err(e) => {
                    failure = Some(format!("download failed: {e}"));
                }
            }
        }
        if let Some(error) = failure {
            tracing::warn!(
                attachment_filename = %att.filename,
                attachment_size = att.size,
                error = %error,
                "discord text attachment download failed for all candidate URLs"
            );
        }
    }
    results
}

fn attachment_download_urls(att: &DiscordAttachment) -> Vec<&str> {
    let mut urls = Vec::new();
    if !att.proxy_url.is_empty() {
        urls.push(att.proxy_url.as_str());
    }
    if !att.url.is_empty() && att.url != att.proxy_url {
        urls.push(att.url.as_str());
    }
    urls
}

/// Enrich message content by inlining text-type attachment content.
///
/// Non-text and oversized attachments are silently skipped.
async fn enrich_content_with_attachments(
    http: &reqwest::Client,
    content: &str,
    attachments: &[DiscordAttachment],
) -> String {
    if attachments.is_empty() {
        return content.to_string();
    }
    let text_files = fetch_text_attachments(http, attachments).await;
    if text_files.is_empty() {
        return content.to_string();
    }
    let mut enriched = content.to_string();
    for (filename, file_content) in &text_files {
        enriched.push_str(&format!(
            "\n\n---\n[Attached file: {filename}]\n{file_content}\n---"
        ));
    }
    enriched
}

impl DiscordTransport {
    /// Create a new Discord transport.
    pub fn new(config: DiscordConfig, api: PawApiClient) -> Self {
        Self {
            config,
            api,
            http: reqwest::Client::new(),
            gateway: GatewayState::new(),
            channel_entity_id: Arc::new(RwLock::new(None)),
            reply_webhook_url: Arc::new(RwLock::new(None)),
            dm_channels: Arc::new(RwLock::new(BTreeMap::new())),
            last_message_cursor: Arc::new(RwLock::new(String::new())),
            typing_cancels: Arc::new(RwLock::new(BTreeMap::new())),
            ready_signal: None,
        }
    }

    /// Attach a notifier that flips to `true` after the first READY event.
    pub fn with_ready_signal(mut self, ready_signal: watch::Sender<bool>) -> Self {
        self.ready_signal = Some(ready_signal);
        self
    }

    /// Run the transport indefinitely.
    pub async fn run(&self) -> Result<(), String> {
        // Phase 1: Start webhook listener for reply delivery.
        let webhook_listener = self.spawn_webhook_listener().await?;
        let webhook_port = webhook_listener.port();
        let webhook_url = format!("http://127.0.0.1:{webhook_port}/reply");
        tracing::info!(
            webhook_port,
            webhook_url = %webhook_url,
            "discord webhook listener started"
        );

        // Phase 2: Bootstrap the Channel entity.
        self.bootstrap_channel(&webhook_url).await?;

        // Phase 2b: Register slash commands (/plan, /execute).
        match fetch_application_id(&self.http, &self.config.bot_token).await {
            Ok(app_id) => {
                if let Err(e) = register_commands(
                    &self.http,
                    &self.config.bot_token,
                    &app_id,
                    self.config.guild_id.as_deref(),
                )
                .await
                {
                    tracing::warn!(error = %e, "discord slash command registration failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "discord application id lookup failed");
            }
        }

        // Phase 3: Connect to Discord Gateway.
        let gateway_url = fetch_gateway_url(&self.http, &self.config.bot_token).await?;
        tracing::info!(gateway_url = %gateway_url, "discord gateway url fetched");

        // Phase 4: Event loop with reconnection.
        let mut backoff = Duration::from_secs(1);
        let mut url = format!("{gateway_url}/?v=10&encoding=json");

        loop {
            match self.connect_and_run(&url).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => {
                    tracing::warn!(
                        gateway_url = %url,
                        backoff_secs = backoff.as_secs(),
                        error = %e,
                        "discord gateway loop failed"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }

            // Flush cursor before reconnecting so it survives if we crash.
            self.flush_cursor().await;

            if let Some(resume) = self.gateway.resume_url.read().await.as_ref() {
                url = format!("{resume}/?v=10&encoding=json");
            }

            tracing::info!(gateway_url = %url, "discord reconnecting");
        }
    }

    /// Bootstrap the Channel entity used by the Discord transport.
    ///
    /// The transport is the SOLE OWNER of Channel entities. The soul bootstrap
    /// does not create, modify, or query Channels — only AgentRoutes and Souls.
    ///
    /// Reuses an existing Connected/Disconnected Channel if one exists
    /// (preserving ChannelSessions and conversation state across restarts).
    /// Archives any duplicates. Only creates a new Channel if none exists.
    async fn bootstrap_channel(&self, webhook_url: &str) -> Result<(), String> {
        *self.reply_webhook_url.write().await = Some(webhook_url.to_string());

        let mut attempt = 0usize;
        let mut backoff = Duration::from_millis(200);

        loop {
            attempt += 1;
            match self.bootstrap_channel_once(webhook_url).await {
                Ok(()) => return Ok(()),
                Err(error) if attempt < 5 && is_retryable_local_odata_bootstrap_error(&error) => {
                    tracing::warn!(
                        attempt,
                        backoff_ms = backoff.as_millis(),
                        error = %error,
                        "discord Channel bootstrap hit a transient local OData error; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(3));
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn bootstrap_channel_once(&self, webhook_url: &str) -> Result<(), String> {
        let existing = self
            .api
            .query_entities("Channels", "ChannelType eq 'discord'", 50)
            .await?;

        // Find the best Channel to reuse: prefer Connected > Disconnected,
        // and among those, the one with the highest message_count (most active).
        let mut best_id = String::new();
        let mut best_msg_count: i64 = -1;
        let mut best_dm_thread_id: Option<String> = None;
        let mut others_to_archive = Vec::new();

        for ch in &existing {
            if crate::entity_is_archived(ch) {
                continue;
            }
            let status = ch
                .get("status")
                .or_else(|| ch.get("fields").and_then(|f| f.get("Status")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let id = ch
                .get("entity_id")
                .or_else(|| ch.get("Id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if id.is_empty() {
                continue;
            }

            if status == "Connected" || status == "Disconnected" {
                let msg_count = ch
                    .get("counters")
                    .and_then(|c| c.get("message_count"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                if msg_count > best_msg_count {
                    if !best_id.is_empty() {
                        others_to_archive.push(best_id.clone());
                    }
                    best_id = id;
                    best_msg_count = msg_count;
                    best_dm_thread_id = known_dm_thread_from_channel_entity(ch);

                    // Read cursor from the best channel
                    if let Some(cursor) = ch
                        .get("fields")
                        .and_then(|f| f.get("last_discord_message_id"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        *self.last_message_cursor.write().await = cursor.to_string();
                    }
                } else {
                    others_to_archive.push(id);
                }
            } else {
                // Created, Connecting — not reusable, archive
                others_to_archive.push(id);
            }
        }

        // Archive duplicates and stale channels
        for old_id in &others_to_archive {
            let _ = self
                .api
                .dispatch_action(
                    "Channels",
                    old_id,
                    "Paw.Channel.Archive",
                    serde_json::json!({}),
                )
                .await;
        }

        let channel_id = if !best_id.is_empty() {
            tracing::info!(
                channel_entity_id = %best_id,
                existing_message_count = best_msg_count,
                archived_duplicate_channels = others_to_archive.len(),
                "discord transport reusing existing channel entity"
            );
            // Update webhook_url so reply delivery uses the current port
            let _ = self
                .api
                .dispatch_action(
                    "Channels",
                    &best_id,
                    "Paw.Channel.UpdateConfig",
                    serde_json::json!({
                        "webhook_url": webhook_url,
                    }),
                )
                .await;
            best_id
        } else {
            // No reusable Channel — create fresh.
            let resp = self
                .api
                .create_entity("Channels", serde_json::json!({"ChannelType": "discord"}))
                .await?;
            let id = resp
                .get("entity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            tracing::info!(
                channel_entity_id = %id,
                archived_duplicate_channels = others_to_archive.len(),
                "discord transport created new channel entity"
            );

            let _ = self
                .api
                .dispatch_action(
                    "Channels",
                    &id,
                    "Paw.Channel.Configure",
                    serde_json::json!({
                        "channel_type": "discord",
                        "channel_id": "discord-gateway",
                        "webhook_url": webhook_url,
                    }),
                )
                .await;

            let _ = self
                .api
                .dispatch_action(
                    "Channels",
                    &id,
                    "Paw.Channel.Connect",
                    serde_json::json!({}),
                )
                .await;

            id
        };

        if channel_id.is_empty() {
            return Err("Failed to bootstrap Channel entity".to_string());
        }

        if let Some(thread_id) = best_dm_thread_id.as_deref() {
            self.seed_dm_channel(thread_id).await;
        }

        *self.channel_entity_id.write().await = Some(channel_id.clone());

        Ok(())
    }

    async fn seed_dm_channel(&self, thread_id: &str) {
        if self.dm_channels.read().await.contains_key(thread_id) {
            return;
        }

        match open_dm_channel_at(
            &self.http,
            &self.config.bot_token,
            DISCORD_API_BASE,
            thread_id,
        )
        .await
        {
            Ok(dm_channel_id) => {
                self.dm_channels
                    .write()
                    .await
                    .insert(thread_id.to_string(), dm_channel_id.clone());
                tracing::info!(
                    thread_id,
                    dm_channel_id,
                    "discord seeded DM channel cache from persisted Channel thread"
                );
            }
            Err(error) => {
                tracing::warn!(
                    thread_id,
                    %error,
                    "discord could not reopen persisted DM thread for catch-up"
                );
            }
        }
    }

    async fn cancel_typing_loop(&self, thread_id: &str) {
        if let Some(cancel_tx) = self.typing_cancels.write().await.remove(thread_id) {
            let _ = cancel_tx.send(true);
        }
    }

    async fn recover_archived_channel_binding(
        &self,
        failed_channel_entity_id: &str,
        error: &str,
    ) -> Result<(), String> {
        if !is_archived_channel_action_error(error) {
            return Err(error.to_string());
        }

        let webhook_url = self.reply_webhook_url.read().await.clone().ok_or_else(|| {
            "discord Channel is archived, but no reply webhook URL is available for recovery"
                .to_string()
        })?;

        tracing::warn!(
            channel_entity_id = %failed_channel_entity_id,
            error = %error,
            "discord Channel binding points at an archived entity; rebootstrap and retry"
        );

        {
            let mut channel_entity_id = self.channel_entity_id.write().await;
            if channel_entity_id.as_deref() == Some(failed_channel_entity_id) {
                *channel_entity_id = None;
            }
        }

        self.bootstrap_channel(&webhook_url).await
    }

    async fn dispatch_receive_message_with_recovery(
        &self,
        params: serde_json::Value,
    ) -> Result<String, String> {
        let channel_entity_id = self.channel_entity_id.read().await.clone().ok_or_else(|| {
            "discord message received before channel bootstrap completed".to_string()
        })?;

        match self
            .api
            .dispatch_action(
                "Channels",
                &channel_entity_id,
                "Paw.Channel.ReceiveMessage",
                params.clone(),
            )
            .await
        {
            Ok(_) => Ok(channel_entity_id),
            Err(error) => {
                self.recover_archived_channel_binding(&channel_entity_id, &error)
                    .await?;

                let recovered_channel_entity_id =
                    self.channel_entity_id.read().await.clone().ok_or_else(|| {
                        "discord Channel recovery completed without a new channel binding"
                            .to_string()
                    })?;

                self.api
                    .dispatch_action(
                        "Channels",
                        &recovered_channel_entity_id,
                        "Paw.Channel.ReceiveMessage",
                        params,
                    )
                    .await
                    .map(|_| recovered_channel_entity_id)
                    .map_err(|retry_error| {
                        format!(
                            "Paw.Channel.ReceiveMessage retry after archived Channel recovery failed: {retry_error}"
                        )
                    })
            }
        }
    }

    /// Catch up on DMs missed while the bot was offline.
    ///
    /// Fetches messages newer than `last_message_cursor` from all DM channels
    /// and dispatches ReceiveMessage for each. Called after Gateway READY.
    async fn catch_up_missed_dms(&self) {
        let cursor = self.last_message_cursor.read().await.clone();
        if cursor.is_empty() {
            // No baseline cursor — first-ever run, nothing to catch up on.
            return;
        }

        let channel_entity_id = self.channel_entity_id.read().await.clone();
        let Some(ref entity_id) = channel_entity_id else {
            return;
        };

        let bot_id = self.gateway.bot_user_id.read().await.clone();
        let rest_dm_channels = fetch_dm_channels(&self.http, &self.config.bot_token).await;
        let seeded_dm_channels = self.dm_channels.read().await.clone();
        let dm_channels = catch_up_dm_sources(&rest_dm_channels, &seeded_dm_channels, &bot_id);
        if dm_channels.is_empty() {
            return;
        }

        let mut total_caught_up = 0u32;

        for (thread_id, dm_channel_id) in &dm_channels {
            // Fetch messages newer than our cursor
            let mut after = cursor.clone();
            loop {
                let messages = fetch_channel_messages(
                    &self.http,
                    &self.config.bot_token,
                    dm_channel_id,
                    &after,
                    100,
                )
                .await;

                if messages.is_empty() {
                    break;
                }

                for msg in &messages {
                    let author_id = msg
                        .get("author")
                        .and_then(|a| a.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let is_bot = msg
                        .get("author")
                        .and_then(|a| a.get("bot"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let msg_id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");

                    // Skip bot's own messages and empty messages
                    if author_id == bot_id || is_bot || content.is_empty() {
                        continue;
                    }

                    let replay_span = tracing::info_span!(
                        "discord.receive",
                        otel.name = "discord.receive",
                        discord.entrypoint = "gateway_dm_replay",
                        discord.message_id = msg_id,
                        discord.author_id = author_id,
                        discord.channel_id = dm_channel_id,
                    );
                    let _replay_guard = replay_span.enter();

                    let username = msg
                        .get("author")
                        .and_then(|a| a.get("username"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");

                    tracing::info!(
                        message_id = msg_id,
                        author_id,
                        username,
                        channel_id = dm_channel_id,
                        thread_id,
                        preview = %truncate(content, 40),
                        "discord catch-up replaying missed dm"
                    );

                    // Track DM channel mapping
                    self.dm_channels
                        .write()
                        .await
                        .insert(author_id.to_string(), dm_channel_id.clone());

                    // Dispatch ReceiveMessage
                    let mut params = serde_json::json!({
                        "message_id": msg_id,
                        "author_id": author_id,
                        "thread_id": author_id,
                        "content": content,
                    });
                    apply_current_trace_context(&mut params);

                    match self.dispatch_receive_message_with_recovery(params).await {
                        Ok(_) => {
                            total_caught_up += 1;
                            let mut c = self.last_message_cursor.write().await;
                            if msg_id > c.as_str() {
                                *c = msg_id.to_string();
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                channel_entity_id = %entity_id,
                                message_id = msg_id,
                                author_id,
                                channel_id = dm_channel_id,
                                error = %e,
                                "discord catch-up dispatch failed"
                            );
                        }
                    }
                }

                let last_id = messages
                    .last()
                    .and_then(|m| m.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if messages.len() < 100 || last_id.is_empty() {
                    break;
                }
                after = last_id.to_string();
            }

            // Rate limit: 200ms between channel fetches
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        if total_caught_up > 0 {
            tracing::info!(
                total_caught_up,
                "discord catch-up completed for missed direct messages"
            );
            // Flush cursor to Channel entity
            self.flush_cursor().await;
        }
    }

    /// Persist the last_message_cursor to the Channel entity via UpdateCursor.
    async fn flush_cursor(&self) {
        let cursor = self.last_message_cursor.read().await.clone();
        if cursor.is_empty() {
            return;
        }
        let channel_entity_id = self.channel_entity_id.read().await.clone();
        let Some(ref entity_id) = channel_entity_id else {
            return;
        };
        if let Err(error) = self
            .api
            .dispatch_action(
                "Channels",
                entity_id,
                "Paw.Channel.UpdateCursor",
                serde_json::json!({ "last_discord_message_id": cursor }),
            )
            .await
            && let Err(recovery_error) = self
                .recover_archived_channel_binding(entity_id, &error)
                .await
        {
            tracing::warn!(
                channel_entity_id = %entity_id,
                error = %error,
                recovery_error = %recovery_error,
                "discord cursor flush failed"
            );
        }
    }

    /// Connect to Gateway and run the event loop.
    async fn connect_and_run(&self, url: &str) -> Result<(), String> {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| format!("WebSocket connect failed: {e}"))?;

        let (mut write, mut read) = ws.split();

        // Wait for Hello (op 10).
        let hello = read_payload(&mut read)
            .await?
            .ok_or("Connection closed before Hello")?;

        if hello.op != GatewayOpcode::Hello as u8 {
            return Err(format!("Expected Hello (op 10), got op {}", hello.op));
        }

        let hello_data: HelloData =
            serde_json::from_value(hello.d.ok_or("Hello missing data field")?)
                .map_err(|e| format!("Failed to parse Hello: {e}"))?;

        let heartbeat_interval = Duration::from_millis(hello_data.heartbeat_interval);

        // Send Identify or Resume.
        let can_resume = self.gateway.session_id.read().await.is_some();
        if can_resume {
            let sid = self.gateway.session_id.read().await.clone().unwrap();
            let seq = self.gateway.sequence.load(Ordering::Relaxed);
            send_resume(&mut write, &self.config.bot_token, &sid, seq).await?;
        } else {
            send_identify(&mut write, &self.config.bot_token, self.config.intents).await?;
        }

        // Send presence.
        let _ = send_presence_online(&mut write).await;

        // Heartbeat ticker.
        let (heartbeat_tx, mut heartbeat_rx) = tokio::sync::mpsc::channel::<()>(1);
        let awaiting_heartbeat_ack = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);
            loop {
                interval.tick().await;
                if heartbeat_tx.send(()).await.is_err() {
                    break;
                }
            }
        });

        // Main event loop.
        loop {
            tokio::select! {
                frame = read.next() => {
                    let Some(frame) = frame else {
                        return Err("Discord gateway closed unexpectedly".to_string());
                    };
                    let frame = frame.map_err(|e| format!("WebSocket read error: {e}"))?;
                    if let Message::Close(close) = &frame {
                        let reason = close
                            .as_ref()
                            .map(|frame| format!("code={} reason={}", frame.code, frame.reason))
                            .unwrap_or_else(|| "no close frame".to_string());
                        return Err(format!("Discord gateway closed: {reason}"));
                    }
                    let Some(payload) = parse_frame(frame)? else {
                        continue;
                    };
                    let should_reconnect = self
                        .handle_payload(payload, &awaiting_heartbeat_ack)
                        .await?;
                    if should_reconnect {
                        return Ok(());
                    }
                }
                Some(()) = heartbeat_rx.recv() => {
                    if awaiting_heartbeat_ack.swap(true, Ordering::SeqCst) {
                        return Err(
                            "Discord gateway heartbeat ACK timeout (possible close code 1006)"
                                .to_string(),
                        );
                    }
                    let s = self.gateway.sequence.load(Ordering::Relaxed);
                    let payload = HeartbeatPayload {
                        op: GatewayOpcode::Heartbeat as u8,
                        d: if s > 0 { Some(s) } else { None },
                    };
                    let json = serde_json::to_string(&payload).unwrap_or_default();
                    write
                        .send(Message::Text(json.into()))
                        .await
                        .map_err(|e| format!("Heartbeat send failed: {e}"))?;
                }
            }
        }
    }

    /// Handle a Gateway payload.
    async fn handle_payload(
        &self,
        payload: GatewayPayload,
        awaiting_heartbeat_ack: &AtomicBool,
    ) -> Result<bool, String> {
        if let Some(s) = payload.s {
            self.gateway.sequence.store(s, Ordering::Relaxed);
        }

        match GatewayOpcode::from_u8(payload.op) {
            Some(GatewayOpcode::Dispatch) => {
                let event_name = payload.t.as_deref().unwrap_or("");
                match event_name {
                    "READY" => {
                        if let Some(d) = payload.d {
                            handle_ready(&self.gateway, d).await?;
                        }
                        if let Some(ready_signal) = self.ready_signal.as_ref() {
                            let _ = ready_signal.send(true);
                        }
                        // Catch up on messages missed while offline.
                        self.catch_up_missed_dms().await;
                    }
                    "MESSAGE_CREATE" => {
                        if let Some(d) = payload.d {
                            self.handle_message_create(d).await;
                        }
                    }
                    _ => {}
                }
                Ok(false)
            }
            Some(GatewayOpcode::HeartbeatAck) => {
                awaiting_heartbeat_ack.store(false, Ordering::SeqCst);
                Ok(false)
            }
            Some(GatewayOpcode::Reconnect) => {
                tracing::info!("discord gateway requested reconnect");
                Ok(true)
            }
            Some(GatewayOpcode::InvalidSession) => {
                let resumable = payload.d.and_then(|v| v.as_bool()).unwrap_or(false);
                if !resumable {
                    *self.gateway.session_id.write().await = None;
                }
                tracing::warn!(resumable, "discord gateway reported an invalid session");
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Handle MESSAGE_CREATE: dispatch Channel.ReceiveMessage via OData API.
    ///
    /// All routing, agent creation, and session management is handled by
    /// the route_message WASM module triggered by Channel.ReceiveMessage.
    async fn handle_message_create(&self, data: serde_json::Value) {
        let msg: MessageCreateData = match serde_json::from_value(data) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "discord message_create payload could not be parsed");
                return;
            }
        };

        // Ignore bot's own messages.
        let bot_id = self.gateway.bot_user_id.read().await.clone();
        if msg.author.id == bot_id || msg.author.bot {
            return;
        }

        // DMs only for now.
        if msg.guild_id.is_some() {
            return;
        }

        let receive_span = tracing::info_span!(
            "discord.receive",
            otel.name = "discord.receive",
            discord.entrypoint = "gateway_dm",
            discord.message_id = %msg.id,
            discord.author_id = %msg.author.id,
            discord.channel_id = %msg.channel_id,
        );
        let _receive_guard = receive_span.enter();

        log_message(
            &msg.id,
            &msg.author.id,
            &msg.author.username,
            &msg.channel_id,
            msg.guild_id.as_deref(),
            &msg.content,
        );

        // Track DM channel → user mapping for reply delivery.
        self.dm_channels
            .write()
            .await
            .insert(msg.author.id.clone(), msg.channel_id.clone());

        // Start typing indicator refresh loop.
        // Sends typing every 8 seconds (Discord expires it at ~10s) until cancelled by reply.
        send_typing(&self.http, &self.config.bot_token, &msg.channel_id).await;
        {
            let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
            self.typing_cancels
                .write()
                .await
                .insert(msg.author.id.clone(), cancel_tx);
            let http = self.http.clone();
            let bot_token = self.config.bot_token.clone();
            let channel_id = msg.channel_id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(8)) => {
                            send_typing(&http, &bot_token, &channel_id).await;
                        }
                        _ = cancel_rx.changed() => {
                            break;
                        }
                    }
                }
            });
        }

        // Fetch text-type attachments and inline their content.
        let enriched_content =
            enrich_content_with_attachments(&self.http, &msg.content, &msg.attachments).await;

        let mut params = serde_json::json!({
            "message_id": msg.id,
            "author_id": msg.author.id,
            "thread_id": msg.author.id,  // DMs use author_id as thread
            "content": enriched_content,
        });
        apply_current_trace_context(&mut params);

        match self.dispatch_receive_message_with_recovery(params).await {
            Ok(channel_entity_id) => {
                tracing::info!(
                    channel_entity_id = %channel_entity_id,
                    message_id = %msg.id,
                    author_id = %msg.author.id,
                    username = %msg.author.username,
                    channel_id = %msg.channel_id,
                    attachment_count = msg.attachments.len(),
                    enriched_content_len = enriched_content.len(),
                    "discord receive_message dispatched"
                );
                // Update cursor to track last processed message for catch-up
                let mut cursor = self.last_message_cursor.write().await;
                if msg.id.as_str() > cursor.as_str() {
                    *cursor = msg.id.clone();
                }
            }
            Err(e) => {
                self.cancel_typing_loop(&msg.author.id).await;
                tracing::warn!(
                    message_id = %msg.id,
                    author_id = %msg.author.id,
                    username = %msg.author.username,
                    channel_id = %msg.channel_id,
                    error = %e,
                    "discord receive_message dispatch failed"
                );
                // Send error message to user.
                let _ = send_discord_message(
                    &self.http,
                    &self.config.bot_token,
                    &msg.channel_id,
                    "Sorry, I encountered an error processing your message.",
                )
                .await;
            }
        }
    }

    /// Start a webhook HTTP listener that receives reply callbacks from
    /// the `send_reply` WASM module and interaction callbacks from Discord.
    /// Returns the bound port.
    ///
    /// Routes:
    /// - POST /reply — receives reply callbacks from send_reply/request_approval WASM
    /// - POST /interaction — receives Discord button click interactions
    async fn spawn_webhook_listener(&self) -> Result<WebhookListenerGuard, String> {
        use super::types::*;
        use axum::{Router, extract::State, routing::post};

        #[derive(Clone)]
        struct WebhookState {
            http: reqwest::Client,
            bot_token: String,
            dm_channels: Arc<RwLock<BTreeMap<String, String>>>,
            api: crate::PawApiClient,
            public_key: String,
            channel_entity_id: Arc<RwLock<Option<String>>>,
            typing_cancels: Arc<RwLock<BTreeMap<String, tokio::sync::watch::Sender<bool>>>>,
        }

        /// Handle reply callbacks from send_reply and request_approval WASM.
        /// Supports optional `components` field for button messages.
        async fn handle_reply(
            State(state): State<WebhookState>,
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> axum::http::StatusCode {
            let thread_id = body.get("thread_id").and_then(|v| v.as_str()).unwrap_or("");
            let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let components: Vec<ActionRow> = body
                .get("components")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let embeds: Vec<Embed> = body
                .get("embeds")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let has_rich_content = !components.is_empty() || !embeds.is_empty();
            let reply_attachments = match parse_reply_attachments(&body) {
                Ok(attachments) => attachments,
                Err(error) => {
                    tracing::error!(%error, "discord reply webhook invalid attachment payload");
                    return axum::http::StatusCode::BAD_REQUEST;
                }
            };
            let has_attachments = !reply_attachments.is_empty();

            if thread_id.is_empty() || (content.is_empty() && !has_rich_content && !has_attachments)
            {
                tracing::error!("discord reply webhook missing content and rich payload");
                return axum::http::StatusCode::BAD_REQUEST;
            }

            // Cancel the typing indicator loop for this thread — reply is arriving.
            if let Some(cancel_tx) = state.typing_cancels.write().await.remove(thread_id) {
                let _ = cancel_tx.send(true);
            }

            // thread_id is the Discord user ID (for DMs). Prefer the warm cache,
            // but reopen the DM channel after reconnects/redeploys if the cache was lost.
            let channel_id = match resolve_dm_channel_id(
                &state.http,
                &state.bot_token,
                &state.dm_channels,
                thread_id,
            )
            .await
            {
                Ok(channel_id) => channel_id,
                Err(error) => {
                    tracing::error!(thread_id, %error, "discord reply webhook could not resolve DM channel");
                    return axum::http::StatusCode::BAD_GATEWAY;
                }
            };

            match deliver_reply_attachments(
                &state.api,
                &state.http,
                &state.bot_token,
                &channel_id,
                content,
                &reply_attachments,
            )
            .await
            {
                Ok(true) => {
                    tracing::info!(
                        thread_id,
                        attachment_count = reply_attachments.len(),
                        "delivered discord reply attachments"
                    );
                    return axum::http::StatusCode::OK;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(thread_id, %error, "discord reply attachment delivery failed");
                    return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                }
            }

            if has_rich_content {
                let fallback_text = build_rich_fallback_text(content, &embeds);
                let needs_fallback = content.len() > 2000 || embeds_exceed_limits(&embeds);

                if !needs_fallback {
                    tracing::info!(
                        thread_id,
                        content_len = content.len(),
                        component_count = components.len(),
                        embed_count = embeds.len(),
                        "delivering rich discord reply"
                    );

                    match send_discord_message_with_components(
                        &state.http,
                        &state.bot_token,
                        &channel_id,
                        content,
                        &components,
                        &embeds,
                    )
                    .await
                    {
                        Ok(_msg) => return axum::http::StatusCode::OK,
                        Err(DiscordApiError::PayloadTooLarge(error)) => {
                            tracing::warn!(thread_id, %error, "rich discord reply exceeded payload limits; falling back to plain text");
                        }
                        Err(error) => {
                            tracing::error!(thread_id, %error, "rich discord reply delivery failed");
                            return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                        }
                    }
                }

                if !fallback_text.is_empty()
                    && let Err(error) = send_discord_message(
                        &state.http,
                        &state.bot_token,
                        &channel_id,
                        &fallback_text,
                    )
                    .await
                {
                    tracing::error!(thread_id, %error, "plain-text fallback delivery failed");
                    return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                }

                if !components.is_empty() {
                    let controls_text = if fallback_text.is_empty() {
                        "Interactive controls:".to_string()
                    } else {
                        "Interactive controls below:".to_string()
                    };
                    if let Err(error) = send_discord_message_with_components(
                        &state.http,
                        &state.bot_token,
                        &channel_id,
                        &controls_text,
                        &components,
                        &[],
                    )
                    .await
                    {
                        tracing::error!(thread_id, %error, "component follow-up delivery failed");
                        return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                    }
                }

                axum::http::StatusCode::OK
            } else {
                tracing::info!(
                    thread_id,
                    content_len = content.len(),
                    "delivering discord reply"
                );

                match send_discord_message(&state.http, &state.bot_token, &channel_id, content)
                    .await
                {
                    Ok(()) => axum::http::StatusCode::OK,
                    Err(e) => {
                        tracing::error!(thread_id, %e, "discord reply delivery failed");
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }
        }

        /// Handle Discord interaction webhooks (button clicks).
        ///
        /// Discord sends INTERACTION_CREATE when a user clicks a button.
        /// We verify the Ed25519 signature, respond with a deferred ack,
        /// then dispatch the Temper action asynchronously.
        async fn handle_interaction(
            State(state): State<WebhookState>,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
            // Verify Ed25519 signature
            if !state.public_key.is_empty() {
                let signature = headers
                    .get("x-signature-ed25519")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let timestamp = headers
                    .get("x-signature-timestamp")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                if !verify_discord_signature(&state.public_key, signature, timestamp, &body) {
                    tracing::warn!(
                        has_signature = !signature.is_empty(),
                        has_timestamp = !timestamp.is_empty(),
                        "discord interaction signature verification failed"
                    );
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"error": "invalid signature"})),
                    );
                }
            }

            let payload: InteractionPayload = match serde_json::from_slice(&body) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "discord interaction payload could not be parsed");
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        axum::Json(serde_json::json!({"error": "invalid payload"})),
                    );
                }
            };

            // Type 1 = PING (Discord verification handshake)
            if payload.interaction_type == 1 {
                tracing::info!("discord interaction ping responded");
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({ "type": 1 })),
                );
            }

            // Type 2 = APPLICATION_COMMAND (slash command)
            if payload.interaction_type == 2 {
                let empty = serde_json::json!({});
                let data = payload.data.as_ref().unwrap_or(&empty);
                let command_name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let command = match command_name {
                    "plan" | "execute" | "reset" => command_name.to_string(),
                    _ => {
                        return (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "type": 4,
                                "data": { "content": "Unknown command.", "flags": 64 }
                            })),
                        );
                    }
                };
                // Extract task/message text from slash command options.
                // /plan and /execute use "task"; /reset uses "message".
                let option_name = if command_name == "reset" {
                    "message"
                } else {
                    "task"
                };
                let task_text = data
                    .get("options")
                    .and_then(|v| v.as_array())
                    .and_then(|opts| {
                        opts.iter()
                            .find(|o| o.get("name").and_then(|n| n.as_str()) == Some(option_name))
                    })
                    .and_then(|o| o.get("value").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();

                // Extract user ID from interaction payload
                let user_id = payload
                    .user
                    .as_ref()
                    .map(|u| u.id.clone())
                    .or_else(|| {
                        payload
                            .member
                            .as_ref()
                            .and_then(|m| m.get("user"))
                            .and_then(|u| u.get("id"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();

                // Store DM channel mapping for reply routing
                let channel_id = payload.channel_id.clone().unwrap_or_default();
                let preview = truncate(&task_text, 80);
                tracing::info!(
                    command = %command,
                    author_id = %user_id,
                    channel_id = %channel_id,
                    preview = %preview,
                    "discord slash command received"
                );
                let receive_span = tracing::info_span!(
                    "discord.receive",
                    otel.name = "discord.receive",
                    discord.entrypoint = "slash_command",
                    discord.command = %command,
                    discord.author_id = %user_id,
                    discord.channel_id = %channel_id,
                );
                if !user_id.is_empty() && !channel_id.is_empty() {
                    state
                        .dm_channels
                        .write()
                        .await
                        .insert(user_id.clone(), channel_id.clone());
                }

                let entity_id = state.channel_entity_id.read().await.clone();
                let api = state.api.clone();
                let http = state.http.clone();
                let bot_token = state.bot_token.clone();
                let app_id = payload.application_id.clone().unwrap_or_default();
                let interaction_token = payload.token.clone();

                // Dispatch ReceiveMessage asynchronously (deferred response)
                tokio::spawn(async move {
                    let Some(entity_id) = entity_id else {
                        tracing::warn!(
                            command = %command,
                            author_id = %user_id,
                            channel_id = %channel_id,
                            "discord slash command received before channel bootstrap completed"
                        );
                        return;
                    };
                    let mut params = serde_json::json!({
                        "message_id": format!("cmd-{}", std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)),
                        "author_id": user_id,
                        "thread_id": user_id,
                        "content": task_text,
                        "command": command,
                    });
                    apply_current_trace_context(&mut params);
                    if let Err(e) = api
                        .dispatch_action(
                            "Channels",
                            &entity_id,
                            "Paw.Channel.ReceiveMessage",
                            params,
                        )
                        .await
                    {
                        tracing::warn!(
                            command = %command,
                            author_id = %user_id,
                            channel_entity_id = %entity_id,
                            error = %e,
                            "discord slash command dispatch failed"
                        );
                        // Edit the deferred message with error
                        let _ = http
                            .patch(format!(
                                "{}/webhooks/{app_id}/{interaction_token}/messages/@original",
                                DISCORD_API_BASE
                            ))
                            .header("Authorization", format!("Bot {bot_token}"))
                            .json(&serde_json::json!({"content": format!("Failed to process /{command}: {e}")}))
                            .send()
                            .await;
                    }
                }
                .instrument(receive_span));

                // Respond with type 5 = DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({ "type": 5 })),
                );
            }

            // Type 3 = MESSAGE_COMPONENT (button click)
            if payload.interaction_type != 3 {
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "type": 4,
                        "data": { "content": "Unsupported interaction type.", "flags": 64 }
                    })),
                );
            }

            let Some(ref data) = payload.data else {
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "type": 4,
                        "data": { "content": "No interaction data.", "flags": 64 }
                    })),
                );
            };

            let custom_id = data.get("custom_id").and_then(|v| v.as_str()).unwrap_or("");
            let parts: Vec<&str> = custom_id.splitn(2, ':').collect();
            if parts.len() != 2 {
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "type": 4,
                        "data": { "content": "Invalid button ID.", "flags": 64 }
                    })),
                );
            }

            let (action, target_id) = (parts[0], parts[1]);
            let is_decision_approval = approval_scope_from_action(action).is_some();
            if !is_decision_approval
                && action != "deny"
                && action != "plan_approve"
                && action != "plan_request_changes"
            {
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({
                        "type": 4,
                        "data": { "content": "Unknown action.", "flags": 64 }
                    })),
                );
            }

            // Extract reviewer info
            let reviewer_id = payload
                .user
                .as_ref()
                .map(|u| u.id.as_str())
                .or_else(|| {
                    payload
                        .member
                        .as_ref()
                        .and_then(|m| m.get("user"))
                        .and_then(|u| u.get("id"))
                        .and_then(|id| id.as_str())
                })
                .unwrap_or("unknown")
                .to_string();

            tracing::info!(
                action,
                target_id,
                reviewer_id,
                channel_id = %payload.channel_id.clone().unwrap_or_default(),
                "discord component interaction received"
            );

            // Process via Temper's native decisions API asynchronously
            let api = state.api.clone();
            let target_id_owned = target_id.to_string();
            let action_owned = action.to_string();
            let reviewer_id_owned = reviewer_id.clone();
            let token = payload.token.clone();
            let app_id = payload.application_id.clone().unwrap_or_default();
            let http = state.http.clone();

            tokio::spawn(async move {
                let base_url = api.config().base_url.clone();
                let tenant = api.config().tenant.clone();

                // Read the original message content so we can preserve it
                let original_content = {
                    let msg_url = format!(
                        "https://discord.com/api/v10/webhooks/{app_id}/{token}/messages/@original"
                    );
                    match http.get(&msg_url).send().await {
                        Ok(resp) => resp
                            .json::<serde_json::Value>()
                            .await
                            .ok()
                            .and_then(|v| {
                                v.get("content").and_then(|c| c.as_str()).map(String::from)
                            })
                            .unwrap_or_default(),
                        Err(_) => String::new(),
                    }
                };

                let (_success, status_line) = match action_owned.as_str() {
                    approval_action if approval_scope_from_action(approval_action).is_some() => {
                        let approve_url = format!(
                            "{base_url}/api/tenants/{tenant}/decisions/{target_id_owned}/approve"
                        );
                        let scope = approval_scope_from_action(approval_action)
                            .expect("checked approval action above");
                        let decision =
                            fetch_pending_decision(&api, &base_url, &tenant, &target_id_owned)
                                .await
                                .ok()
                                .flatten();
                        match approval_body_for_scope(
                            scope,
                            decision.as_ref(),
                            format!("discord:{reviewer_id_owned}"),
                        ) {
                            Ok(body) => match api.raw_post(&approve_url, body).await {
                                Ok(_) => {
                                    (true, format!("Approval recorded by <@{reviewer_id_owned}>"))
                                }
                                Err(e) => (false, format!("Approval failed: {e}")),
                            },
                            Err(e) => (false, format!("Approval failed: {e}")),
                        }
                    }
                    "deny" => {
                        let deny_url = format!(
                            "{base_url}/api/tenants/{tenant}/decisions/{target_id_owned}/deny"
                        );
                        let deny_body = serde_json::json!({
                            "decided_by": format!("discord:{reviewer_id_owned}")
                        });
                        match api.raw_post(&deny_url, deny_body).await {
                            Ok(_) => (true, format!("Denial recorded by <@{reviewer_id_owned}>")),
                            Err(e) => (false, format!("Deny failed: {e}")),
                        }
                    }
                    "plan_approve" => match api
                        .dispatch_action(
                            "Plans",
                            &target_id_owned,
                            "TemperPaw.Approve",
                            serde_json::json!({}),
                        )
                        .await
                    {
                        Ok(_) => (true, format!("Plan approved by <@{reviewer_id_owned}>")),
                        Err(e) => (false, format!("Plan approval failed: {e}")),
                    },
                    "plan_request_changes" => {
                        let review_notes = format!(
                            "Changes requested by discord:{reviewer_id_owned}. Review the plan, revise it, and resubmit for approval."
                        );
                        match api
                            .dispatch_action(
                                "Plans",
                                &target_id_owned,
                                "TemperPaw.RequestChanges",
                                serde_json::json!({ "review_notes": review_notes }),
                            )
                            .await
                        {
                            Ok(_) => (
                                true,
                                format!(
                                    "Plan changes requested by <@{reviewer_id_owned}>. Additional details can be sent in-thread."
                                ),
                            ),
                            Err(e) => (false, format!("Request changes failed: {e}")),
                        }
                    }
                    _ => (false, "Unknown action.".to_string()),
                };

                // Build the updated message: original context + decision result
                let message = if original_content.is_empty() {
                    status_line
                } else {
                    let updated = if original_content.contains("**Plan Review Required**") {
                        original_content.replace(
                            "**Plan Review Required**",
                            &format!("~~Plan Review Required~~ **{status_line}**"),
                        )
                    } else {
                        original_content.replace(
                            "**Permission Required**",
                            &format!("~~Permission Required~~ **{status_line}**"),
                        )
                    };
                    // Remove the "Click a button" instruction line if present
                    updated
                        .lines()
                        .filter(|l| !l.contains("Click a button"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };

                // Session resume/fail is now handled by GovernanceDecision.DispatchCallback
                // effect — no transport-level orchestration needed.

                // Edit the Discord message to show result
                if !app_id.is_empty() && !token.is_empty() {
                    let follow_up_url = format!(
                        "https://discord.com/api/v10/webhooks/{app_id}/{token}/messages/@original"
                    );
                    let _ = http
                        .patch(&follow_up_url)
                        .header("Content-Type", "application/json")
                        .json(&serde_json::json!({
                            "content": message,
                            "components": []
                        }))
                        .send()
                        .await;
                }
            });

            // Respond with deferred update (type 6 = DEFERRED_UPDATE_MESSAGE)
            // This removes the "thinking" state and we'll edit the message in the spawn above.
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({ "type": 6 })),
            )
        }

        let webhook_state = WebhookState {
            http: self.http.clone(),
            bot_token: self.config.bot_token.clone(),
            dm_channels: self.dm_channels.clone(),
            api: self.api.clone(),
            public_key: self.config.public_key.clone(),
            channel_entity_id: self.channel_entity_id.clone(),
            typing_cancels: self.typing_cancels.clone(),
        };

        /// Handle typing indicator requests from WASM modules.
        async fn handle_typing(
            State(state): State<WebhookState>,
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> axum::http::StatusCode {
            let thread_id = body.get("thread_id").and_then(|v| v.as_str()).unwrap_or("");
            if thread_id.is_empty() {
                return axum::http::StatusCode::BAD_REQUEST;
            }
            let channel_id = match resolve_dm_channel_id(
                &state.http,
                &state.bot_token,
                &state.dm_channels,
                thread_id,
            )
            .await
            {
                Ok(channel_id) => channel_id,
                Err(error) => {
                    tracing::warn!(thread_id, %error, "discord typing webhook could not resolve DM channel");
                    return axum::http::StatusCode::BAD_GATEWAY;
                }
            };
            send_typing(&state.http, &state.bot_token, &channel_id).await;
            axum::http::StatusCode::OK
        }

        let app = Router::new()
            .route("/reply", post(handle_reply))
            .route("/interaction", post(handle_interaction))
            .route("/typing", post(handle_typing))
            .with_state(webhook_state);

        let port = self.config.webhook_port;
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .map_err(|e| format!("Failed to bind webhook listener: {e}"))?;
        let actual_port = listener
            .local_addr()
            .map_err(|e| format!("Failed to get listener address: {e}"))?
            .port();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
            {
                tracing::warn!(error = %e, "discord webhook listener stopped with an error");
            }
        });

        Ok(WebhookListenerGuard::new(actual_port, shutdown_tx, task))
    }
}

/// Verify a Discord interaction signature using Ed25519.
///
/// Discord sends X-Signature-Ed25519 and X-Signature-Timestamp headers.
/// The signed message is: timestamp + body.
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
        Ok(b) => b,
        Err(_) => return false,
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&pk_bytes) else {
        return false;
    };

    let Ok(sig_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    let sig_bytes: [u8; 64] = match sig_bytes.try_into() {
        Ok(b) => b,
        Err(_) => return false,
    };
    let signature = Signature::from_bytes(&sig_bytes);

    let mut message = Vec::with_capacity(timestamp.len() + body.len());
    message.extend_from_slice(timestamp.as_bytes());
    message.extend_from_slice(body);

    verifying_key.verify(&message, &signature).is_ok()
}

fn is_retryable_local_odata_bootstrap_error(error: &str) -> bool {
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
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn is_archived_channel_action_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("not valid from state 'archived'")
        || error.contains("not valid from state ''archived''")
}

/// Read one Gateway payload from the WebSocket with timeout.
async fn read_payload(read: &mut WsStream) -> Result<Option<GatewayPayload>, String> {
    let frame = tokio::time::timeout(Duration::from_secs(60), read.next())
        .await
        .map_err(|_| "Timed out waiting for Gateway payload".to_string())?;
    let Some(frame) = frame else {
        return Ok(None);
    };
    let frame = frame.map_err(|e| format!("WebSocket read error: {e}"))?;
    parse_frame(frame)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    #[tokio::test]
    async fn webhook_listener_guard_releases_port_on_drop() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route("/ping", get(|| async { "ok" }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let guard = WebhookListenerGuard::new(port, shutdown_tx, task);
        drop(guard);
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rebound = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await;
        assert!(
            rebound.is_ok(),
            "expected port {port} to be reusable after drop"
        );
    }

    #[derive(Clone, Default)]
    struct BootstrapProbe {
        update_config_body: Arc<Mutex<Option<serde_json::Value>>>,
    }

    #[derive(Clone, Default)]
    struct ArchivedRecoveryProbe {
        retry_count: Arc<AtomicUsize>,
    }

    async fn spawn_test_server(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn bootstrap_reused_channel_refreshes_webhook_url() {
        let probe = BootstrapProbe::default();
        let app = Router::new()
            .route(
                "/tdata/Channels",
                get(
                    |Query(_query): Query<std::collections::HashMap<String, String>>| async move {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "value": [{
                                    "entity_id": "ch_existing",
                                    "status": "Disconnected",
                                    "fields": {
                                        "WebhookUrl": "",
                                        "last_discord_message_id": "123"
                                    },
                                    "counters": {
                                        "message_count": 42
                                    }
                                }]
                            })),
                        )
                    },
                ),
            )
            .route(
                "/tdata/Channels('ch_existing')/Paw.Channel.UpdateConfig",
                post(
                    |State(probe): State<BootstrapProbe>, Json(body): Json<serde_json::Value>| async move {
                        *probe.update_config_body.lock().unwrap() = Some(body);
                        (StatusCode::OK, Json(json!({"ok": true})))
                    },
                ),
            )
            .with_state(probe.clone());

        let base_url = spawn_test_server(app).await;
        let api = crate::PawApiClient::new(crate::PawApiConfig {
            base_url,
            tenant: "default".to_string(),
            api_key: None,
        });
        let transport = DiscordTransport::new(
            DiscordConfig {
                bot_token: "token".to_string(),
                intents: intents::DEFAULT,
                webhook_port: 3488,
                public_key: "public-key".to_string(),
                guild_id: None,
                feed_channel_id: None,
                forum_channel_id: None,
            },
            api,
        );

        transport
            .bootstrap_channel("http://127.0.0.1:3488/reply")
            .await
            .expect("reused channel bootstrap should succeed");

        let update = probe
            .update_config_body
            .lock()
            .unwrap()
            .clone()
            .expect("expected UpdateConfig to be dispatched");
        assert_eq!(
            update.get("webhook_url").and_then(|value| value.as_str()),
            Some("http://127.0.0.1:3488/reply"),
            "reused discord channels must refresh the reply webhook target"
        );
    }

    #[tokio::test]
    async fn receive_message_reboots_archived_channel_binding_and_retries() {
        let probe = ArchivedRecoveryProbe::default();
        let app = Router::new()
            .route(
                "/tdata/Channels",
                get(
                    |Query(_query): Query<std::collections::HashMap<String, String>>| async move {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "value": [{
                                    "entity_id": "ch_recovered",
                                    "status": "Connected",
                                    "fields": {
                                        "WebhookUrl": "",
                                        "last_discord_message_id": "123"
                                    },
                                    "counters": {
                                        "message_count": 7
                                    }
                                }]
                            })),
                        )
                    },
                ),
            )
            .route(
                "/tdata/Channels('ch_recovered')/Paw.Channel.UpdateConfig",
                post(|| async { (StatusCode::OK, Json(json!({"ok": true}))) }),
            )
            .route(
                "/tdata/Channels('ch_archived')/Paw.Channel.ReceiveMessage",
                post(|| async {
                    (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": {
                                "code": "ActionFailed",
                                "message": "Action 'ReceiveMessage' not valid from state 'Archived'"
                            }
                        })),
                    )
                }),
            )
            .route(
                "/tdata/Channels('ch_recovered')/Paw.Channel.ReceiveMessage",
                post(|State(probe): State<ArchivedRecoveryProbe>| async move {
                    probe.retry_count.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(json!({"ok": true})))
                }),
            )
            .with_state(probe.clone());

        let base_url = spawn_test_server(app).await;
        let api = crate::PawApiClient::new(crate::PawApiConfig {
            base_url,
            tenant: "default".to_string(),
            api_key: None,
        });
        let transport = DiscordTransport::new(
            DiscordConfig {
                bot_token: "token".to_string(),
                intents: intents::DEFAULT,
                webhook_port: 3488,
                public_key: "public-key".to_string(),
                guild_id: None,
                feed_channel_id: None,
                forum_channel_id: None,
            },
            api,
        );
        *transport.channel_entity_id.write().await = Some("ch_archived".to_string());
        *transport.reply_webhook_url.write().await =
            Some("http://127.0.0.1:3488/reply".to_string());

        let channel_id = transport
            .dispatch_receive_message_with_recovery(json!({
                "message_id": "msg_1",
                "author_id": "user_1",
                "thread_id": "user_1",
                "content": "hello"
            }))
            .await
            .expect("archived Channel recovery should retry ReceiveMessage");

        assert_eq!(channel_id, "ch_recovered");
        assert_eq!(
            transport.channel_entity_id.read().await.as_deref(),
            Some("ch_recovered")
        );
        assert_eq!(
            probe.retry_count.load(Ordering::SeqCst),
            1,
            "expected ReceiveMessage to be retried against the recovered Channel"
        );
    }

    #[tokio::test]
    async fn resolve_dm_channel_id_reopens_and_caches_missing_dm_mapping() {
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_route = open_attempts.clone();
        let app = Router::new().route(
            "/users/@me/channels",
            post(move |Json(body): Json<serde_json::Value>| {
                let attempts = attempts_for_route.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        body.get("recipient_id").and_then(|value| value.as_str()),
                        Some("user_456")
                    );
                    (StatusCode::OK, Json(json!({ "id": "dm_reopened" })))
                }
            }),
        );
        let base_url = spawn_test_server(app).await;
        let dm_channels = Arc::new(RwLock::new(BTreeMap::new()));

        let channel_id = resolve_dm_channel_id_at(
            &reqwest::Client::new(),
            "bot-token",
            &dm_channels,
            "user_456",
            &base_url,
        )
        .await
        .expect("missing DM cache entries should be reopened through Discord REST");

        assert_eq!(channel_id, "dm_reopened");
        assert_eq!(
            dm_channels.read().await.get("user_456").map(String::as_str),
            Some("dm_reopened")
        );

        let cached_id = resolve_dm_channel_id_at(
            &reqwest::Client::new(),
            "bot-token",
            &dm_channels,
            "user_456",
            &base_url,
        )
        .await
        .expect("cached DM channel should be reused");

        assert_eq!(cached_id, "dm_reopened");
        assert_eq!(
            open_attempts.load(Ordering::SeqCst),
            1,
            "expected only one Discord open-DM REST call"
        );
    }

    #[tokio::test]
    async fn bootstrap_channel_retries_transient_create_failure() {
        let create_attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_route = create_attempts.clone();
        let app = Router::new()
            .route(
                "/tdata/Channels",
                get(
                    |Query(_query): Query<std::collections::HashMap<String, String>>| async move {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "value": []
                            })),
                        )
                    },
                )
                .post(move || {
                    let attempts = attempts_for_route.clone();
                    async move {
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                        if attempt == 1 {
                            return (
                                StatusCode::SERVICE_UNAVAILABLE,
                                Json(json!({"error": "store warming up"})),
                            );
                        }

                        (
                            StatusCode::CREATED,
                            Json(json!({"entity_id": "ch_retry", "ChannelType": "discord"})),
                        )
                    }
                }),
            )
            .route(
                "/tdata/Channels('ch_retry')/Paw.Channel.Configure",
                post(|| async { (StatusCode::OK, Json(json!({"ok": true}))) }),
            )
            .route(
                "/tdata/Channels('ch_retry')/Paw.Channel.Connect",
                post(|| async { (StatusCode::OK, Json(json!({"ok": true}))) }),
            );

        let base_url = spawn_test_server(app).await;
        let api = crate::PawApiClient::new(crate::PawApiConfig {
            base_url,
            tenant: "default".to_string(),
            api_key: None,
        });
        let transport = DiscordTransport::new(
            DiscordConfig {
                bot_token: "token".to_string(),
                intents: intents::DEFAULT,
                webhook_port: 3488,
                public_key: "public-key".to_string(),
                guild_id: None,
                feed_channel_id: None,
                forum_channel_id: None,
            },
            api,
        );

        transport
            .bootstrap_channel("http://127.0.0.1:3488/reply")
            .await
            .expect("transient Channel creation failures should be retried");

        assert_eq!(
            create_attempts.load(Ordering::SeqCst),
            2,
            "expected one failed create attempt followed by a retry"
        );
    }
}
