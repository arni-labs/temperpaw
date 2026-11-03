//! Discord Gateway API types.
//!
//! Covers the subset of the Discord Gateway v10 protocol needed for
//! receiving messages and sending replies. Only DM support initially.

use serde::{Deserialize, Serialize};

// ── Gateway opcodes ──────────────────────────────────────────────────

/// Discord Gateway opcodes (v10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GatewayOpcode {
    /// Server → Client: dispatched event (MESSAGE_CREATE, READY, etc.).
    Dispatch = 0,
    /// Client → Server: heartbeat ping.
    Heartbeat = 1,
    /// Client → Server: identify payload with token + intents.
    Identify = 2,
    /// Client → Server: update bot presence/status.
    PresenceUpdate = 3,
    /// Client → Server: resume a dropped session.
    Resume = 6,
    /// Server → Client: reconnect request.
    Reconnect = 7,
    /// Server → Client: invalid session.
    InvalidSession = 9,
    /// Server → Client: hello with heartbeat interval.
    Hello = 10,
    /// Server → Client: heartbeat ACK.
    HeartbeatAck = 11,
}

impl GatewayOpcode {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Dispatch),
            1 => Some(Self::Heartbeat),
            2 => Some(Self::Identify),
            3 => Some(Self::PresenceUpdate),
            6 => Some(Self::Resume),
            7 => Some(Self::Reconnect),
            9 => Some(Self::InvalidSession),
            10 => Some(Self::Hello),
            11 => Some(Self::HeartbeatAck),
            _ => None,
        }
    }
}

// ── Gateway payloads ─────────────────────────────────────────────────

/// Raw gateway payload envelope.
#[derive(Debug, Deserialize)]
pub struct GatewayPayload {
    /// Opcode.
    pub op: u8,
    /// Event data (opcode-dependent).
    pub d: Option<serde_json::Value>,
    /// Sequence number (only for op 0 Dispatch).
    pub s: Option<u64>,
    /// Event name (only for op 0 Dispatch, e.g. "MESSAGE_CREATE").
    pub t: Option<String>,
}

/// Hello payload (op 10).
#[derive(Debug, Deserialize)]
pub struct HelloData {
    /// Heartbeat interval in milliseconds.
    pub heartbeat_interval: u64,
}

/// Ready payload (op 0, t = "READY").
#[derive(Debug, Deserialize)]
pub struct ReadyData {
    /// The bot's user object.
    pub user: DiscordUser,
    /// Session ID for resuming.
    pub session_id: String,
    /// Gateway URL for resuming.
    pub resume_gateway_url: String,
}

// ── Discord object types ─────────────────────────────────────────────

/// Minimal Discord user object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

/// A file attachment on a Discord message.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordAttachment {
    /// Attachment snowflake ID.
    pub id: String,
    /// Original filename.
    pub filename: String,
    /// File size in bytes.
    pub size: u64,
    /// Source URL (CDN).
    pub url: String,
    /// Proxied URL (more reliable for fetching).
    pub proxy_url: String,
    /// MIME type (e.g. "text/plain", "image/png").
    #[serde(default)]
    pub content_type: Option<String>,
}

/// MESSAGE_CREATE event data.
#[derive(Debug, Deserialize)]
pub struct MessageCreateData {
    /// Message ID.
    pub id: String,
    /// Channel ID where the message was sent.
    pub channel_id: String,
    /// Author of the message.
    pub author: DiscordUser,
    /// Message content.
    pub content: String,
    /// Guild ID (None for DMs).
    #[serde(default)]
    pub guild_id: Option<String>,
    /// File attachments.
    #[serde(default)]
    pub attachments: Vec<DiscordAttachment>,
}

// ── Outbound payloads (Client → Server) ──────────────────────────────

/// Identify payload (op 2).
#[derive(Debug, Serialize)]
pub struct IdentifyPayload {
    pub op: u8,
    pub d: IdentifyData,
}

#[derive(Debug, Serialize)]
pub struct IdentifyData {
    pub token: String,
    pub intents: u32,
    pub properties: ConnectionProperties,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence: Option<PresenceUpdateData>,
}

/// Presence update data (used in IDENTIFY and opcode 3).
#[derive(Debug, Serialize)]
pub struct PresenceUpdateData {
    /// Unix time (ms) when the client went idle, or null if not idle.
    pub since: Option<u64>,
    /// Bot activities (status text).
    pub activities: Vec<PresenceActivity>,
    /// Status: "online", "dnd", "idle", "invisible", "offline".
    pub status: String,
    /// Whether the client is AFK.
    pub afk: bool,
}

/// A single presence activity entry.
#[derive(Debug, Serialize)]
pub struct PresenceActivity {
    /// Activity name displayed in Discord.
    pub name: String,
    /// Activity type: 0=Playing, 1=Streaming, 2=Listening, 3=Watching, 5=Competing.
    #[serde(rename = "type")]
    pub activity_type: u8,
}

#[derive(Debug, Serialize)]
pub struct ConnectionProperties {
    pub os: String,
    pub browser: String,
    pub device: String,
}

/// Resume payload (op 6).
#[derive(Debug, Serialize)]
pub struct ResumePayload {
    pub op: u8,
    pub d: ResumeData,
}

#[derive(Debug, Serialize)]
pub struct ResumeData {
    pub token: String,
    pub session_id: String,
    pub seq: u64,
}

/// Heartbeat payload (op 1).
#[derive(Debug, Serialize)]
pub struct HeartbeatPayload {
    pub op: u8,
    pub d: Option<u64>,
}

// ── Discord Gateway Intents ──────────────────────────────────────────

/// Privileged + non-privileged intents needed for DM message reception.
pub mod intents {
    /// Required for guild membership visibility.
    pub const GUILDS: u32 = 1 << 0;
    /// Receive events for messages in guild text channels.
    pub const GUILD_MESSAGES: u32 = 1 << 9;
    /// Receive events for DM messages.
    pub const DIRECT_MESSAGES: u32 = 1 << 12;
    /// Access message content (privileged intent, must be enabled in Developer Portal).
    pub const MESSAGE_CONTENT: u32 = 1 << 15;

    /// Default intents for the channel transport: DMs + guild messages + content.
    pub const DEFAULT: u32 = GUILDS | GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT;
}

// ── REST API types (for sending messages) ────────────────────────────

/// POST /channels/{channel_id}/messages request body.
#[derive(Debug, Serialize)]
pub struct CreateMessageRequest {
    pub content: String,
}

// ── Message Components (buttons, action rows, embeds) ────────────────

/// POST /channels/{channel_id}/messages request body with components and embeds.
#[derive(Debug, Serialize)]
pub struct CreateMessageWithComponents {
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ActionRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub embeds: Vec<Embed>,
}

// ── Discord Embeds ───────────────────────────────────────────────────

/// A rich embed card in a Discord message.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Embed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Decimal RGB color for the left sidebar. See `embed_colors`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<EmbedField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<EmbedFooter>,
}

/// A field within an embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

/// Footer text at the bottom of an embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedFooter {
    pub text: String,
}

/// Standard embed colors for agent messages.
pub mod embed_colors {
    /// Green — success, approved.
    pub const SUCCESS: u32 = 0x57F287;
    /// Red — error, denied.
    pub const ERROR: u32 = 0xED4245;
    /// Blurple — agent activity, info.
    pub const INFO: u32 = 0x5865F2;
    /// Yellow — warning, pending.
    pub const WARNING: u32 = 0xFEE75C;
    /// Grey — idle, inactive.
    pub const IDLE: u32 = 0x99AAB5;
}

/// An action row containing interactive components (buttons, selects).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRow {
    /// Always 1 for action rows.
    #[serde(rename = "type")]
    pub component_type: u8,
    pub components: Vec<Button>,
}

/// A clickable button component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Button {
    /// Always 2 for buttons.
    #[serde(rename = "type")]
    pub component_type: u8,
    /// 1=Primary (blurple), 2=Secondary (grey), 3=Success (green), 4=Danger (red).
    pub style: u8,
    pub label: String,
    pub custom_id: String,
}

impl Button {
    /// Create a green "Approve" button.
    pub fn approve(approval_id: &str) -> Self {
        Self {
            component_type: 2,
            style: 3,
            label: "Approve".to_string(),
            custom_id: format!("approve:{approval_id}"),
        }
    }

    /// Create a red "Deny" button.
    pub fn deny(approval_id: &str) -> Self {
        Self {
            component_type: 2,
            style: 4,
            label: "Deny".to_string(),
            custom_id: format!("deny:{approval_id}"),
        }
    }
}

impl ActionRow {
    /// Create an action row with approval buttons.
    pub fn approval_buttons(approval_id: &str) -> Self {
        Self {
            component_type: 1,
            components: vec![Button::approve(approval_id), Button::deny(approval_id)],
        }
    }
}

// ── Discord Interactions (button clicks) ─────────────────────────────

/// Incoming interaction payload from Discord (button click, slash command, etc.).
#[derive(Debug, Deserialize)]
pub struct InteractionPayload {
    /// Interaction ID.
    pub id: String,
    /// Interaction type: 1=PING, 2=APPLICATION_COMMAND, 3=MESSAGE_COMPONENT.
    #[serde(rename = "type")]
    pub interaction_type: u8,
    /// Interaction data — shape depends on interaction_type.
    /// Type 2 (slash command): has `name`, `options`.
    /// Type 3 (button click): has `custom_id`, `component_type`.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Interaction token for follow-up messages.
    pub token: String,
    /// The user who triggered the interaction.
    #[serde(default)]
    pub member: Option<serde_json::Value>,
    #[serde(default)]
    pub user: Option<DiscordUser>,
    /// Application ID.
    #[serde(default)]
    pub application_id: Option<String>,
    /// Channel ID where the interaction was triggered.
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// Response to a Discord interaction.
#[derive(Debug, Serialize)]
pub struct InteractionResponse {
    /// 1=PONG, 4=CHANNEL_MESSAGE_WITH_SOURCE, 5=DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE, 6=DEFERRED_UPDATE_MESSAGE.
    #[serde(rename = "type")]
    pub response_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<InteractionCallbackData>,
}

/// Optional data for an interaction response.
#[derive(Debug, Serialize)]
pub struct InteractionCallbackData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<ActionRow>>,
    /// 64 = EPHEMERAL (only visible to the user who clicked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
}

// ── REST API types (other) ───────────────────────────────────────────

/// GET /gateway/bot response.
#[derive(Debug, Deserialize)]
pub struct GatewayBotResponse {
    pub url: String,
    #[serde(default)]
    pub shards: u32,
}

/// Response from creating a forum post thread.
#[derive(Debug, Deserialize)]
pub struct ForumThreadResponse {
    /// The thread (channel) ID.
    pub id: String,
}
