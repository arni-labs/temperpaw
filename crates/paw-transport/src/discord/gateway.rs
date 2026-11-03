//! Discord Gateway WebSocket lifecycle — connect, heartbeat, identify, resume.
//!
//! Pure platform I/O. No Paw business logic here.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use futures_util::SinkExt;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

use super::types::*;

/// Discord REST API v10 base URL.
pub(crate) const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Discord Gateway connection state.
pub(crate) struct GatewayState {
    /// Bot's own user ID (populated after READY).
    pub bot_user_id: Arc<RwLock<String>>,
    /// Last sequence number received.
    pub sequence: Arc<AtomicU64>,
    /// Session ID for resume (populated after READY).
    pub session_id: Arc<RwLock<Option<String>>>,
    /// Resume gateway URL (populated after READY).
    pub resume_url: Arc<RwLock<Option<String>>>,
}

#[derive(Debug)]
pub enum DiscordApiError {
    PayloadTooLarge(String),
    RequestFailed(String),
}

impl fmt::Display for DiscordApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge(message) | Self::RequestFailed(message) => f.write_str(message),
        }
    }
}

impl GatewayState {
    pub fn new() -> Self {
        Self {
            bot_user_id: Arc::new(RwLock::new(String::new())),
            sequence: Arc::new(AtomicU64::new(0)),
            session_id: Arc::new(RwLock::new(None)),
            resume_url: Arc::new(RwLock::new(None)),
        }
    }
}

/// Fetch the Gateway bot URL from Discord REST API.
pub(crate) async fn fetch_gateway_url(
    http: &reqwest::Client,
    bot_token: &str,
) -> Result<String, String> {
    let resp = http
        .get(format!("{DISCORD_API_BASE}/gateway/bot"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch gateway URL: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Gateway bot endpoint returned {status}: {body}"));
    }

    let bot_resp: GatewayBotResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse gateway response: {e}"))?;

    Ok(bot_resp.url)
}

/// Type alias for the WebSocket write half.
pub(crate) type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Type alias for the WebSocket read half.
pub(crate) type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Send Identify payload with presence.
pub(crate) async fn send_identify(
    write: &mut WsSink,
    bot_token: &str,
    intents: u32,
) -> Result<(), String> {
    let identify = IdentifyPayload {
        op: GatewayOpcode::Identify as u8,
        d: IdentifyData {
            token: bot_token.to_string(),
            intents,
            properties: ConnectionProperties {
                os: "linux".to_string(),
                browser: "paw".to_string(),
                device: "paw".to_string(),
            },
            presence: Some(PresenceUpdateData {
                since: None,
                activities: vec![],
                status: "online".to_string(),
                afk: false,
            }),
        },
    };
    let json = serde_json::to_string(&identify)
        .map_err(|e| format!("Failed to serialize Identify: {e}"))?;
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("Identify send failed: {e}"))?;
    Ok(())
}

/// Send Resume payload.
pub(crate) async fn send_resume(
    write: &mut WsSink,
    bot_token: &str,
    session_id: &str,
    sequence: u64,
) -> Result<(), String> {
    let resume = ResumePayload {
        op: GatewayOpcode::Resume as u8,
        d: ResumeData {
            token: bot_token.to_string(),
            session_id: session_id.to_string(),
            seq: sequence,
        },
    };
    let json =
        serde_json::to_string(&resume).map_err(|e| format!("Failed to serialize Resume: {e}"))?;
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("Resume send failed: {e}"))?;
    Ok(())
}

/// Send presence update (opcode 3).
pub(crate) async fn send_presence_online(write: &mut WsSink) -> Result<(), String> {
    let presence = serde_json::json!({
        "op": 3,
        "d": { "since": null, "activities": [], "status": "online", "afk": false }
    });
    let json = serde_json::to_string(&presence).unwrap_or_default();
    write
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("Presence send failed: {e}"))?;
    Ok(())
}

/// Parse a WebSocket frame into a Gateway payload.
pub(crate) fn parse_frame(frame: Message) -> Result<Option<GatewayPayload>, String> {
    let text = match frame {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => {
            String::from_utf8(b.to_vec()).map_err(|e| format!("Invalid UTF-8: {e}"))?
        }
        Message::Close(_) => return Ok(None),
        _ => return Ok(None),
    };
    let payload: GatewayPayload =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse payload: {e}"))?;
    Ok(Some(payload))
}

/// Handle READY event: extract bot user ID and session info.
pub(crate) async fn handle_ready(
    state: &GatewayState,
    data: serde_json::Value,
) -> Result<(), String> {
    let ready: ReadyData =
        serde_json::from_value(data).map_err(|e| format!("Failed to parse READY: {e}"))?;

    tracing::info!(
        bot_user_id = %ready.user.id,
        username = %ready.user.username,
        discriminator = %ready.user.discriminator.as_deref().unwrap_or("0"),
        "discord gateway ready"
    );

    *state.bot_user_id.write().await = ready.user.id;
    *state.session_id.write().await = Some(ready.session_id);
    *state.resume_url.write().await = Some(ready.resume_gateway_url);

    Ok(())
}

/// Send a message to a Discord channel via REST API.
pub async fn send_discord_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
) -> Result<(), DiscordApiError> {
    let chunks = split_message(content, 2000);
    for chunk in chunks {
        let body = CreateMessageRequest {
            content: chunk.to_string(),
        };
        discord_post_message(http, bot_token, channel_id, &body).await?;
    }
    Ok(())
}

/// Send a message with interactive components and/or embeds to a Discord channel.
/// Returns the full message response JSON (includes message ID for later edits).
pub async fn send_discord_message_with_components(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    components: &[super::types::ActionRow],
    embeds: &[super::types::Embed],
) -> Result<serde_json::Value, DiscordApiError> {
    let body = super::types::CreateMessageWithComponents {
        content: content.to_string(),
        components: components.to_vec(),
        embeds: embeds.to_vec(),
    };
    let resp = discord_post_message(http, bot_token, channel_id, &body).await?;

    resp.json::<serde_json::Value>().await.map_err(|e| {
        DiscordApiError::RequestFailed(format!("Failed to parse Discord message response: {e}"))
    })
}

#[derive(Debug, Clone)]
pub struct DiscordFileUpload {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Send a message with file attachments to a Discord channel via multipart upload.
pub async fn send_discord_message_with_files(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    files: &[DiscordFileUpload],
) -> Result<(), DiscordApiError> {
    if files.is_empty() {
        return send_discord_message(http, bot_token, channel_id, content).await;
    }

    let url = format!("{DISCORD_API_BASE}/channels/{channel_id}/messages");
    let payload_json = serde_json::json!({
        "content": content,
        "attachments": files
            .iter()
            .enumerate()
            .map(|(idx, file)| serde_json::json!({
                "id": idx.to_string(),
                "filename": file.filename.as_str(),
            }))
            .collect::<Vec<_>>(),
    });

    let mut form = reqwest::multipart::Form::new().text("payload_json", payload_json.to_string());
    for (idx, file) in files.iter().enumerate() {
        let part = reqwest::multipart::Part::bytes(file.bytes.clone())
            .file_name(file.filename.clone())
            .mime_str(&file.content_type)
            .map_err(|err| {
                DiscordApiError::RequestFailed(format!(
                    "invalid Discord upload content type {}: {err}",
                    file.content_type
                ))
            })?;
        let field_name = if idx == 0 {
            "files[0]".to_string()
        } else {
            format!("files[{idx}]")
        };
        form = form.part(field_name, part);
    }

    let resp = http
        .post(&url)
        .header("Authorization", format!("Bot {bot_token}"))
        .multipart(form)
        .send()
        .await
        .map_err(|err| DiscordApiError::RequestFailed(format!("Discord API error: {err}")))?;

    if resp.status().is_success() {
        return Ok(());
    }

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        || (status == reqwest::StatusCode::BAD_REQUEST && is_payload_too_large_body(&body_text))
    {
        return Err(DiscordApiError::PayloadTooLarge(format!(
            "Discord API returned {status}: {body_text}"
        )));
    }

    Err(DiscordApiError::RequestFailed(format!(
        "Discord API returned {status}: {body_text}"
    )))
}

async fn discord_post_message<T: serde::Serialize>(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    body: &T,
) -> Result<reqwest::Response, DiscordApiError> {
    let url = format!("{DISCORD_API_BASE}/channels/{channel_id}/messages");
    let mut attempts = 0;

    loop {
        let resp = http
            .post(&url)
            .header("Authorization", format!("Bot {bot_token}"))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| DiscordApiError::RequestFailed(format!("Discord API error: {e}")))?;

        if resp.status().is_success() {
            return Ok(resp);
        }

        let status = resp.status();
        let retry_after = resp
            .headers()
            .get("Retry-After")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0)
            .clamp(0.5, 30.0);
        let body_text = resp.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && attempts < 3 {
            attempts += 1;
            tokio::time::sleep(Duration::from_secs_f64(retry_after)).await;
            continue;
        }

        if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
            || (status == reqwest::StatusCode::BAD_REQUEST && is_payload_too_large_body(&body_text))
        {
            return Err(DiscordApiError::PayloadTooLarge(format!(
                "Discord API returned {status}: {body_text}"
            )));
        }

        return Err(DiscordApiError::RequestFailed(format!(
            "Discord API returned {status}: {body_text}"
        )));
    }
}

fn is_payload_too_large_body(body: &str) -> bool {
    body.contains("BASE_TYPE_MAX_LENGTH")
        || body.contains("Must be 2000 or fewer in length")
        || body.contains("must be 6000 or fewer in length")
        || body.contains("Invalid Form Body")
}

/// Edit a Discord message to update content and/or remove components.
pub async fn edit_discord_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    message_id: &str,
    content: &str,
    components: &[super::types::ActionRow],
) -> Result<(), String> {
    let body = serde_json::json!({
        "content": content,
        "components": components,
    });

    let resp = http
        .patch(format!(
            "{DISCORD_API_BASE}/channels/{channel_id}/messages/{message_id}"
        ))
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Discord edit error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Discord edit returned {status}: {body}"));
    }

    Ok(())
}

/// Send typing indicator.
pub(crate) async fn send_typing(http: &reqwest::Client, bot_token: &str, channel_id: &str) {
    let _ = http
        .post(format!("{DISCORD_API_BASE}/channels/{channel_id}/typing"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await;
}

/// Fetch the bot's DM channels from Discord REST API.
pub(crate) async fn fetch_dm_channels(
    http: &reqwest::Client,
    bot_token: &str,
) -> Vec<serde_json::Value> {
    let resp = http
        .get(format!("{DISCORD_API_BASE}/users/@me/channels"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            r.json::<Vec<serde_json::Value>>().await.unwrap_or_default()
        }
        _ => vec![],
    }
}

pub(crate) async fn open_dm_channel_at(
    http: &reqwest::Client,
    bot_token: &str,
    api_base: &str,
    recipient_id: &str,
) -> Result<String, DiscordApiError> {
    if recipient_id.is_empty() {
        return Err(DiscordApiError::RequestFailed(
            "Discord DM recipient_id is empty".to_string(),
        ));
    }

    let url = format!("{}/users/@me/channels", api_base.trim_end_matches('/'));
    let resp = http
        .post(url)
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "recipient_id": recipient_id }))
        .send()
        .await
        .map_err(|e| DiscordApiError::RequestFailed(format!("Discord open DM error: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(DiscordApiError::RequestFailed(format!(
            "Discord open DM returned {status}: {body}"
        )));
    }

    let body = resp.json::<serde_json::Value>().await.map_err(|e| {
        DiscordApiError::RequestFailed(format!("Failed to parse Discord open DM response: {e}"))
    })?;

    body.get("id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            DiscordApiError::RequestFailed(
                "Discord open DM response missing channel id".to_string(),
            )
        })
}

/// Fetch messages from a Discord channel newer than `after_id`.
/// Returns messages in chronological order (oldest first).
pub(crate) async fn fetch_channel_messages(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    after_id: &str,
    limit: u32,
) -> Vec<serde_json::Value> {
    let url =
        format!("{DISCORD_API_BASE}/channels/{channel_id}/messages?after={after_id}&limit={limit}");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            // Discord returns messages newest-first; reverse for chronological order
            let mut msgs: Vec<serde_json::Value> = r.json().await.unwrap_or_default();
            msgs.reverse();
            msgs
        }
        _ => vec![],
    }
}

/// UTF-8 safe truncation.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max);
        format!("{}...", &s[..end])
    }
}

/// Split a message into chunks that fit within Discord's character limit.
/// UTF-8 safe — uses floor_char_boundary to avoid splitting multi-byte chars.
fn split_message(content: &str, max_len: usize) -> Vec<&str> {
    if content.len() <= max_len {
        return vec![content];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }

        let boundary = remaining.floor_char_boundary(max_len);
        let split_at = remaining[..boundary].rfind('\n').unwrap_or(boundary);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk);
        remaining = rest.trim_start_matches('\n');
    }

    chunks
}

/// Send a message to a guild channel (for #feed).
///
/// Posts a message with optional embeds to a Discord text channel.
pub async fn send_channel_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    embeds: Vec<super::types::Embed>,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "content": content,
        "embeds": embeds,
    });

    let resp = http
        .post(format!("{DISCORD_API_BASE}/channels/{channel_id}/messages"))
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Discord API error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Discord API returned {status}: {body}"));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse Discord message response: {e}"))
}

/// Create a forum post (thread with initial message).
///
/// Posts to a Discord forum channel, creating a new thread with the given
/// name, initial message content, embeds, and optional tag IDs.
pub async fn create_forum_post(
    http: &reqwest::Client,
    bot_token: &str,
    forum_channel_id: &str,
    name: &str,
    content: &str,
    embeds: Vec<super::types::Embed>,
    applied_tags: Vec<String>,
) -> Result<super::types::ForumThreadResponse, String> {
    let body = serde_json::json!({
        "name": name,
        "message": {
            "content": content,
            "embeds": embeds,
        },
        "applied_tags": applied_tags,
    });

    let resp = http
        .post(format!(
            "{DISCORD_API_BASE}/channels/{forum_channel_id}/threads"
        ))
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Discord API error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Discord API returned {status}: {body}"));
    }

    resp.json::<super::types::ForumThreadResponse>()
        .await
        .map_err(|e| format!("Failed to parse forum thread response: {e}"))
}

/// Send a message to an existing thread.
///
/// Threads in Discord are channels, so this posts to the thread's channel ID.
pub async fn send_thread_message(
    http: &reqwest::Client,
    bot_token: &str,
    thread_id: &str,
    content: &str,
    embeds: Vec<super::types::Embed>,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "content": content,
        "embeds": embeds,
    });

    let resp = http
        .post(format!("{DISCORD_API_BASE}/channels/{thread_id}/messages"))
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Discord API error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Discord API returned {status}: {body}"));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse Discord message response: {e}"))
}

/// Edit an existing message (for updating forum post top embed).
///
/// Patches a message's content and/or embeds. Fields set to `None` are left unchanged.
pub async fn edit_message(
    http: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    message_id: &str,
    content: Option<&str>,
    embeds: Option<Vec<super::types::Embed>>,
) -> Result<serde_json::Value, String> {
    let mut body = serde_json::Map::new();
    if let Some(c) = content {
        body.insert(
            "content".to_string(),
            serde_json::Value::String(c.to_string()),
        );
    }
    if let Some(e) = embeds {
        body.insert(
            "embeds".to_string(),
            serde_json::to_value(e).unwrap_or_default(),
        );
    }

    let resp = http
        .patch(format!(
            "{DISCORD_API_BASE}/channels/{channel_id}/messages/{message_id}"
        ))
        .header("Authorization", format!("Bot {bot_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Discord edit error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Discord edit returned {status}: {body}"));
    }

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse Discord edit response: {e}"))
}

/// Log a truncated message from a user through the tracing pipeline.
pub(crate) fn log_message(
    message_id: &str,
    author_id: &str,
    username: &str,
    channel_id: &str,
    guild_id: Option<&str>,
    content: &str,
) {
    tracing::info!(
        message_id,
        author_id,
        username,
        channel_id,
        guild_id = guild_id.unwrap_or("dm"),
        preview = %truncate(content, 80),
        content_len = content.len(),
        "discord message received"
    );
}

/// Fetch the bot's application ID from Discord REST API.
pub(crate) async fn fetch_application_id(
    http: &reqwest::Client,
    bot_token: &str,
) -> Result<String, String> {
    let resp = http
        .get(format!("{DISCORD_API_BASE}/applications/@me"))
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch application info: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GET /applications/@me returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse application info: {e}"))?;

    body.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Application info missing 'id' field".to_string())
}

/// Register `/plan`, `/execute`, and `/reset` slash commands with Discord.
///
/// Uses guild-scoped commands if `guild_id` is provided (instant propagation,
/// good for development). Falls back to global commands (up to 1 hour to propagate).
pub(crate) async fn register_commands(
    http: &reqwest::Client,
    bot_token: &str,
    application_id: &str,
    guild_id: Option<&str>,
) -> Result<(), String> {
    let commands = serde_json::json!([
        {
            "name": "plan",
            "description": "Start a task in plan mode (research & plan before implementing)",
            "type": 1,
            "options": [{
                "name": "task",
                "description": "What to plan",
                "type": 3,
                "required": true
            }]
        },
        {
            "name": "execute",
            "description": "Switch to execute mode (full tool access)",
            "type": 1,
            "options": [{
                "name": "task",
                "description": "Optional task to execute",
                "type": 3,
                "required": false
            }]
        },
        {
            "name": "reset",
            "description": "Start a fresh conversation (clears history)",
            "type": 1,
            "options": [{
                "name": "message",
                "description": "Optional first message for the new conversation",
                "type": 3,
                "required": false
            }]
        }
    ]);

    let url = if let Some(gid) = guild_id {
        format!("{DISCORD_API_BASE}/applications/{application_id}/guilds/{gid}/commands")
    } else {
        format!("{DISCORD_API_BASE}/applications/{application_id}/commands")
    };

    let resp = http
        .put(&url)
        .header("Authorization", format!("Bot {bot_token}"))
        .json(&commands)
        .send()
        .await
        .map_err(|e| format!("Failed to register slash commands: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("PUT commands returned {status}: {body}"));
    }

    let scope = guild_id
        .map(|g| format!("guild {g}"))
        .unwrap_or("global".to_string());
    tracing::info!(scope = %scope, "discord slash commands registered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::{Json, Router, routing::post};
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone, Default)]
    struct OpenDmProbe {
        authorization: Arc<Mutex<Option<String>>>,
        recipient_id: Arc<Mutex<Option<String>>>,
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
    async fn open_dm_channel_posts_recipient_and_returns_channel_id() {
        let probe = OpenDmProbe::default();
        let app = Router::new()
            .route(
                "/users/@me/channels",
                post(
                    |State(probe): State<OpenDmProbe>,
                     headers: HeaderMap,
                     Json(body): Json<serde_json::Value>| async move {
                        *probe.authorization.lock().unwrap() = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string);
                        *probe.recipient_id.lock().unwrap() = body
                            .get("recipient_id")
                            .and_then(|value| value.as_str())
                            .map(str::to_string);

                        (StatusCode::OK, Json(json!({ "id": "dm_channel_123" })))
                    },
                ),
            )
            .with_state(probe.clone());
        let base_url = spawn_test_server(app).await;

        let channel_id =
            open_dm_channel_at(&reqwest::Client::new(), "bot-token", &base_url, "user_456")
                .await
                .expect("opening a DM channel should return the Discord channel id");

        assert_eq!(channel_id, "dm_channel_123");
        assert_eq!(
            probe.authorization.lock().unwrap().as_deref(),
            Some("Bot bot-token")
        );
        assert_eq!(
            probe.recipient_id.lock().unwrap().as_deref(),
            Some("user_456")
        );
    }
}
