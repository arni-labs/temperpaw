//! Slack transport — wires Slack Socket Mode to Paw Channel entities.
//!
//! On startup: bootstraps the Channel entity used for Slack delivery.
//! On message events: dispatches Channel.ReceiveMessage via OData API.
//! On Channel.SendReply events: delivers reply via Slack Web API.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use super::api;
use super::socket;
use super::types::*;
use crate::{
    PawApiClient, approval_body_for_scope, approval_scope_from_action, fetch_pending_decision,
};

/// Configuration for the Slack transport.
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Slack App-Level Token (xapp-...) for Socket Mode connection.
    pub app_token: String,
    /// Slack Bot Token (xoxb-...) for Web API calls.
    pub bot_token: String,
    /// Port for the webhook listener (receives replies from send_reply WASM).
    /// Defaults to 0 (auto-assign).
    pub webhook_port: u16,
    /// Slack Signing Secret for webhook signature verification (HMAC-SHA256).
    pub signing_secret: String,
}

/// Slack channel transport.
///
/// Connects to Slack via Socket Mode, dispatches messages to Paw Channel
/// entities via the OData API, and delivers replies via Slack Web API.
pub struct SlackTransport {
    config: SlackConfig,
    api: PawApiClient,
    http: reqwest::Client,
    /// Channel entity ID in Paw (populated on startup).
    channel_entity_id: Arc<RwLock<Option<String>>>,
    /// Bot's own user ID (populated after auth.test call).
    bot_user_id: Arc<RwLock<String>>,
}

struct WebhookListenerGuard {
    port: u16,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
struct SlackTransportEvent<'a> {
    operation: &'a str,
    outcome: &'a str,
    channel_id: &'a str,
    message_id: &'a str,
    user_id: &'a str,
    command: &'a str,
    webhook_port: Option<u16>,
    message_len: Option<usize>,
    block_count: Option<usize>,
    error: Option<&'a str>,
}

fn log_slack_transport_event(event: SlackTransportEvent<'_>) {
    tracing::info!(
        observability_event = "temperpaw.transport",
        transport.name = "slack",
        transport.operation = event.operation,
        transport.outcome = event.outcome,
        transport.channel_id = event.channel_id,
        transport.message_id = event.message_id,
        transport.command = event.command,
        transport.webhook_port = event.webhook_port.unwrap_or_default(),
        slack.user_id = event.user_id,
        message.length = event.message_len.unwrap_or_default(),
        slack.block_count = event.block_count.unwrap_or_default(),
        error.message = event.error.unwrap_or_default(),
        "slack transport event"
    );
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

impl SlackTransport {
    /// Create a new Slack transport.
    pub fn new(config: SlackConfig, api: PawApiClient) -> Self {
        Self {
            config,
            api,
            http: reqwest::Client::new(),
            channel_entity_id: Arc::new(RwLock::new(None)),
            bot_user_id: Arc::new(RwLock::new(String::new())),
        }
    }

    /// Run the transport indefinitely.
    pub async fn run(&self) -> Result<(), String> {
        // Phase 1: Start webhook listener for reply delivery.
        let webhook_listener = self.spawn_webhook_listener().await?;
        let webhook_port = webhook_listener.port();
        let webhook_url = format!("http://127.0.0.1:{webhook_port}/reply");
        log_slack_transport_event(SlackTransportEvent {
            operation: "webhook_listener",
            outcome: "ready",
            channel_id: "",
            message_id: "",
            user_id: "",
            command: "",
            webhook_port: Some(webhook_port),
            message_len: None,
            block_count: None,
            error: None,
        });

        // Phase 2: Bootstrap the Channel entity.
        self.bootstrap_channel(&webhook_url).await?;

        // Phase 3: Get bot user ID via auth.test.
        match api::auth_test(&self.http, &self.config.bot_token).await {
            Ok(resp) => {
                let user_id = resp.user_id.unwrap_or_default();
                let user_name = resp.user.unwrap_or_default();
                tracing::info!(
                    observability_event = "temperpaw.transport",
                    transport.name = "slack",
                    transport.operation = "auth_test",
                    transport.outcome = "success",
                    slack.user_id = %user_id,
                    slack.user_name = %user_name,
                    "slack transport authenticated"
                );
                *self.bot_user_id.write().await = user_id;
            }
            Err(e) => {
                log_slack_transport_event(SlackTransportEvent {
                    operation: "auth_test",
                    outcome: "error",
                    channel_id: "",
                    message_id: "",
                    user_id: "",
                    command: "",
                    webhook_port: None,
                    message_len: None,
                    block_count: None,
                    error: Some(&e),
                });
                return Err(format!("Slack auth.test failed: {e}"));
            }
        }

        // Phase 4: Connect Socket Mode and run event loop with reconnection.
        let mut backoff = Duration::from_secs(1);

        loop {
            let socket_url =
                match socket::fetch_socket_url(&self.http, &self.config.app_token).await {
                    Ok(url) => url,
                    Err(e) => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "socket_url",
                            outcome: "error",
                            channel_id: "",
                            message_id: "",
                            user_id: "",
                            command: "",
                            webhook_port: None,
                            message_len: None,
                            block_count: None,
                            error: Some(&e),
                        });
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(60));
                        continue;
                    }
                };

            match self.connect_and_handle(&socket_url).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => {
                    log_slack_transport_event(SlackTransportEvent {
                        operation: "socket_loop",
                        outcome: "error",
                        channel_id: "",
                        message_id: "",
                        user_id: "",
                        command: "",
                        webhook_port: None,
                        message_len: None,
                        block_count: None,
                        error: Some(&e),
                    });
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }

            log_slack_transport_event(SlackTransportEvent {
                operation: "socket_reconnect",
                outcome: "scheduled",
                channel_id: "",
                message_id: "",
                user_id: "",
                command: "",
                webhook_port: None,
                message_len: None,
                block_count: None,
                error: None,
            });
        }
    }

    /// Bootstrap the Channel entity used by the Slack transport.
    async fn bootstrap_channel(&self, webhook_url: &str) -> Result<(), String> {
        // Archive any stale Channel entities from previous runs.
        let stale = self
            .api
            .query_entities("Channels", "ChannelType eq 'slack'", 50)
            .await
            .unwrap_or_default();
        for old in &stale {
            if crate::entity_is_archived(old) {
                continue;
            }
            if let Some(old_id) = old
                .get("Id")
                .or_else(|| old.get("entity_id"))
                .and_then(|v| v.as_str())
            {
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
        }

        let channel_id = {
            // Create new Channel entity.
            let resp = self
                .api
                .create_entity(
                    "Channels",
                    serde_json::json!({
                        "ChannelType": "slack",
                    }),
                )
                .await?;
            let id = resp
                .get("entity_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            log_slack_transport_event(SlackTransportEvent {
                operation: "channel_bootstrap",
                outcome: "created",
                channel_id: &id,
                message_id: "",
                user_id: "",
                command: "",
                webhook_port: None,
                message_len: None,
                block_count: None,
                error: None,
            });

            // Configure the channel with webhook for reply delivery.
            let _ = self
                .api
                .dispatch_action(
                    "Channels",
                    &id,
                    "Paw.Channel.Configure",
                    serde_json::json!({
                        "channel_type": "slack",
                        "channel_id": "slack-socket-mode",
                        "webhook_url": webhook_url,
                    }),
                )
                .await;

            // Connect → Ready.
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

        *self.channel_entity_id.write().await = Some(channel_id);

        Ok(())
    }

    /// Connect to Socket Mode and handle events.
    async fn connect_and_handle(&self, url: &str) -> Result<(), String> {
        let channel_entity_id = self.channel_entity_id.clone();
        let bot_user_id = self.bot_user_id.clone();
        let api = self.api.clone();
        let http = self.http.clone();
        let bot_token = self.config.bot_token.clone();

        socket::connect_and_run(url, |envelope| {
            let channel_entity_id = channel_entity_id.clone();
            let bot_user_id = bot_user_id.clone();
            let api = api.clone();
            let http = http.clone();
            let bot_token = bot_token.clone();

            async move {
                match envelope.envelope_type.as_str() {
                    "events_api" => {
                        handle_events_api(
                            envelope.payload,
                            &channel_entity_id,
                            &bot_user_id,
                            &api,
                            &http,
                            &bot_token,
                        )
                        .await;
                    }
                    "interactive" => {
                        handle_interactive(envelope.payload, &api, &http, &bot_token).await;
                    }
                    "slash_commands" => {
                        handle_slash_command(envelope.payload, &channel_entity_id, &api).await;
                    }
                    other => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "envelope",
                            outcome: "ignored",
                            channel_id: "",
                            message_id: "",
                            user_id: "",
                            command: other,
                            webhook_port: None,
                            message_len: None,
                            block_count: None,
                            error: None,
                        });
                    }
                }
            }
        })
        .await
    }

    /// Start a webhook HTTP listener that receives reply callbacks from
    /// the `send_reply` WASM module. Returns the bound port.
    async fn spawn_webhook_listener(&self) -> Result<WebhookListenerGuard, String> {
        use axum::{Router, extract::State, routing::post};

        #[derive(Clone)]
        struct WebhookState {
            http: reqwest::Client,
            bot_token: String,
        }

        /// Handle reply callbacks from send_reply WASM.
        async fn handle_reply(
            State(state): State<WebhookState>,
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> axum::http::StatusCode {
            let thread_id = body.get("thread_id").and_then(|v| v.as_str()).unwrap_or("");
            let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");

            if thread_id.is_empty() || content.is_empty() {
                log_slack_transport_event(SlackTransportEvent {
                    operation: "send_reply",
                    outcome: "invalid",
                    channel_id: thread_id,
                    message_id: "",
                    user_id: "",
                    command: "",
                    webhook_port: None,
                    message_len: Some(content.len()),
                    block_count: None,
                    error: Some("missing thread_id or content"),
                });
                return axum::http::StatusCode::BAD_REQUEST;
            }

            // In Slack, thread_id is the channel ID (DM or otherwise).
            // No channel mapping needed — Slack provides the channel directly.
            let channel_id = thread_id;

            // Check for Block Kit blocks.
            let blocks: Vec<SlackBlock> = body
                .get("blocks")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let has_blocks = !blocks.is_empty();

            if has_blocks {
                match api::send_slack_message_with_blocks(
                    &state.http,
                    &state.bot_token,
                    channel_id,
                    content,
                    &blocks,
                )
                .await
                {
                    Ok(_) => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "send_reply",
                            outcome: "success",
                            channel_id,
                            message_id: "",
                            user_id: "",
                            command: "",
                            webhook_port: None,
                            message_len: Some(content.len()),
                            block_count: Some(blocks.len()),
                            error: None,
                        });
                        axum::http::StatusCode::OK
                    }
                    Err(e) => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "send_reply",
                            outcome: "error",
                            channel_id,
                            message_id: "",
                            user_id: "",
                            command: "",
                            webhook_port: None,
                            message_len: Some(content.len()),
                            block_count: Some(blocks.len()),
                            error: Some(&e),
                        });
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            } else {
                match api::send_slack_message(&state.http, &state.bot_token, channel_id, content)
                    .await
                {
                    Ok(()) => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "send_reply",
                            outcome: "success",
                            channel_id,
                            message_id: "",
                            user_id: "",
                            command: "",
                            webhook_port: None,
                            message_len: Some(content.len()),
                            block_count: None,
                            error: None,
                        });
                        axum::http::StatusCode::OK
                    }
                    Err(e) => {
                        log_slack_transport_event(SlackTransportEvent {
                            operation: "send_reply",
                            outcome: "error",
                            channel_id,
                            message_id: "",
                            user_id: "",
                            command: "",
                            webhook_port: None,
                            message_len: Some(content.len()),
                            block_count: None,
                            error: Some(&e),
                        });
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }
        }

        let webhook_state = WebhookState {
            http: self.http.clone(),
            bot_token: self.config.bot_token.clone(),
        };

        let app = Router::new()
            .route("/reply", post(handle_reply))
            .with_state(webhook_state);

        let port = self.config.webhook_port;
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .map_err(|e| format!("Failed to bind Slack webhook listener: {e}"))?;
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
                log_slack_transport_event(SlackTransportEvent {
                    operation: "webhook_listener",
                    outcome: "error",
                    channel_id: "",
                    message_id: "",
                    user_id: "",
                    command: "",
                    webhook_port: None,
                    message_len: None,
                    block_count: None,
                    error: Some(&e.to_string()),
                });
            }
        });

        Ok(WebhookListenerGuard::new(actual_port, shutdown_tx, task))
    }
}

/// Handle an Events API envelope — extract the inner message event and
/// dispatch Channel.ReceiveMessage.
async fn handle_events_api(
    payload: serde_json::Value,
    channel_entity_id: &Arc<RwLock<Option<String>>>,
    bot_user_id: &Arc<RwLock<String>>,
    api: &PawApiClient,
    http: &reqwest::Client,
    bot_token: &str,
) {
    // Extract the inner event from the Events API wrapper.
    let events_payload: EventsApiPayload = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "parse_events_api",
                outcome: "error",
                channel_id: "",
                message_id: "",
                user_id: "",
                command: "",
                webhook_port: None,
                message_len: None,
                block_count: None,
                error: Some(&e.to_string()),
            });
            return;
        }
    };

    let event_type = events_payload
        .event
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if event_type != "message" {
        return;
    }

    let msg: SlackMessageEvent = match serde_json::from_value(events_payload.event) {
        Ok(m) => m,
        Err(e) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "parse_message_event",
                outcome: "error",
                channel_id: "",
                message_id: "",
                user_id: "",
                command: "",
                webhook_port: None,
                message_len: None,
                block_count: None,
                error: Some(&e.to_string()),
            });
            return;
        }
    };

    // Skip bot messages and message subtypes (edits, deletes, etc.).
    if msg.subtype.is_some() || msg.bot_id.is_some() {
        return;
    }

    let user_id = match &msg.user {
        Some(u) => u.clone(),
        None => return,
    };

    // Ignore bot's own messages.
    let my_id = bot_user_id.read().await.clone();
    if user_id == my_id {
        return;
    }

    // DMs only for now (channel_type = "im").
    let channel_type = msg.channel_type.as_deref().unwrap_or("");
    if channel_type != "im" {
        return;
    }

    let text = msg.text.unwrap_or_default();
    let channel = msg.channel.unwrap_or_default();
    let ts = msg.ts.unwrap_or_default();

    if text.is_empty() || channel.is_empty() {
        return;
    }

    // Dispatch Channel.ReceiveMessage — the WASM handles everything else.
    let entity_id = channel_entity_id.read().await.clone();
    let Some(entity_id) = entity_id else {
        log_slack_transport_event(SlackTransportEvent {
            operation: "receive_message",
            outcome: "error",
            channel_id: &channel,
            message_id: &ts,
            user_id: &user_id,
            command: "",
            webhook_port: None,
            message_len: Some(text.len()),
            block_count: None,
            error: Some("no channel entity bootstrapped"),
        });
        return;
    };

    // For Slack DMs, use the channel ID as the thread_id.
    // This is the correct channel for reply delivery (no mapping needed).
    let params = serde_json::json!({
        "message_id": ts,
        "author_id": user_id,
        "thread_id": channel,
        "content": text,
    });

    match api
        .dispatch_action("Channels", &entity_id, "Paw.Channel.ReceiveMessage", params)
        .await
    {
        Ok(_) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "receive_message",
                outcome: "success",
                channel_id: &channel,
                message_id: &ts,
                user_id: &user_id,
                command: "",
                webhook_port: None,
                message_len: Some(text.len()),
                block_count: None,
                error: None,
            });
        }
        Err(e) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "receive_message",
                outcome: "error",
                channel_id: &channel,
                message_id: &ts,
                user_id: &user_id,
                command: "",
                webhook_port: None,
                message_len: Some(text.len()),
                block_count: None,
                error: Some(&e),
            });
            // Send error message to user.
            let _ = api::send_slack_message(
                http,
                bot_token,
                &channel,
                "Sorry, I encountered an error processing your message.",
            )
            .await;
        }
    }
}

/// Handle an interactive envelope (button clicks for approve/deny).
async fn handle_interactive(
    payload: serde_json::Value,
    api: &PawApiClient,
    http: &reqwest::Client,
    bot_token: &str,
) {
    let interaction: SlackInteractionPayload = match serde_json::from_value(payload) {
        Ok(p) => p,
        Err(e) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "approval_interaction",
                outcome: "error",
                channel_id: "",
                message_id: "",
                user_id: "",
                command: "",
                webhook_port: None,
                message_len: None,
                block_count: None,
                error: Some(&e.to_string()),
            });
            return;
        }
    };

    if interaction.interaction_type != "block_actions" {
        return;
    }

    let Some(action) = interaction.actions.first() else {
        return;
    };

    let action_id = &action.action_id;
    let parts: Vec<&str> = action_id.splitn(2, ':').collect();
    if parts.len() != 2 {
        log_slack_transport_event(SlackTransportEvent {
            operation: "approval_interaction",
            outcome: "invalid",
            channel_id: "",
            message_id: "",
            user_id: &interaction.user.id,
            command: action_id,
            webhook_port: None,
            message_len: None,
            block_count: None,
            error: Some("invalid action_id format"),
        });
        return;
    }

    let (action_type, target_id) = (parts[0], parts[1]);
    let is_decision_approval = approval_scope_from_action(action_type).is_some();
    if !is_decision_approval
        && action_type != "deny"
        && action_type != "plan_approve"
        && action_type != "plan_request_changes"
    {
        return;
    }

    let reviewer_id = interaction.user.id.clone();
    let reviewer_name = interaction
        .user
        .name
        .as_deref()
        .or(interaction.user.username.as_deref())
        .unwrap_or("unknown");

    tracing::info!(
        observability_event = "temperpaw.transport",
        transport.name = "slack",
        transport.operation = "approval_interaction",
        transport.outcome = "received",
        transport.command = %action_type,
        approval.target_id = %target_id,
        slack.user_id = %reviewer_id,
        slack.user_name = %reviewer_name,
        "slack approval interaction received"
    );

    let base_url = api.config().base_url.clone();
    let tenant = api.config().tenant.clone();
    let (_success, status_line) = match action_type {
        approval_action if approval_scope_from_action(approval_action).is_some() => {
            let approve_url =
                format!("{base_url}/api/tenants/{tenant}/decisions/{target_id}/approve");
            let scope =
                approval_scope_from_action(approval_action).expect("checked approval action above");
            let decision = fetch_pending_decision(api, &base_url, &tenant, target_id)
                .await
                .ok()
                .flatten();
            match approval_body_for_scope(scope, decision.as_ref(), format!("slack:{reviewer_id}"))
            {
                Ok(body) => match api.raw_post(&approve_url, body).await {
                    Ok(_) => (true, format!("Approval recorded by <@{reviewer_id}>")),
                    Err(e) => (false, format!("Approval failed: {e}")),
                },
                Err(e) => (false, format!("Approval failed: {e}")),
            }
        }
        "deny" => {
            let deny_url = format!("{base_url}/api/tenants/{tenant}/decisions/{target_id}/deny");
            let deny_body = serde_json::json!({
                "decided_by": format!("slack:{reviewer_id}")
            });
            match api.raw_post(&deny_url, deny_body).await {
                Ok(_) => (true, format!("Denial recorded by <@{reviewer_id}>")),
                Err(e) => (false, format!("Deny failed: {e}")),
            }
        }
        "plan_approve" => match api
            .dispatch_action(
                "Plans",
                target_id,
                "TemperPaw.Approve",
                serde_json::json!({}),
            )
            .await
        {
            Ok(_) => (true, format!("Plan approved by <@{reviewer_id}>")),
            Err(e) => (false, format!("Plan approval failed: {e}")),
        },
        "plan_request_changes" => {
            let review_notes = format!(
                "Changes requested by slack:{reviewer_id}. Review the plan, revise it, and resubmit for approval."
            );
            match api
                .dispatch_action(
                    "Plans",
                    target_id,
                    "TemperPaw.RequestChanges",
                    serde_json::json!({ "review_notes": review_notes }),
                )
                .await
            {
                Ok(_) => (
                    true,
                    format!(
                        "Plan changes requested by <@{reviewer_id}>. Additional details can be sent in-thread."
                    ),
                ),
                Err(e) => (false, format!("Request changes failed: {e}")),
            }
        }
        _ => (false, "Unknown action.".to_string()),
    };

    // Session resume/fail is now handled by GovernanceDecision.DispatchCallback
    // effect — no transport-level orchestration needed.

    // Update the original message to show the decision result.
    let channel_id = interaction
        .channel
        .as_ref()
        .map(|c| c.id.as_str())
        .unwrap_or("");
    let message_ts = interaction
        .message
        .as_ref()
        .and_then(|m| m.get("ts"))
        .and_then(|ts| ts.as_str())
        .unwrap_or("");

    if !channel_id.is_empty() && !message_ts.is_empty() {
        log_slack_transport_event(SlackTransportEvent {
            operation: "approval_interaction",
            outcome: if _success { "success" } else { "error" },
            channel_id,
            message_id: message_ts,
            user_id: &reviewer_id,
            command: action_type,
            webhook_port: None,
            message_len: None,
            block_count: None,
            error: if _success {
                None
            } else {
                Some(status_line.as_str())
            },
        });

        // Update with result text and remove buttons.
        let result_blocks = vec![SlackBlock::Section {
            text: SlackTextObject::mrkdwn(&status_line),
            block_id: None,
            fields: None,
        }];
        if let Err(e) = api::update_slack_message(
            http,
            bot_token,
            channel_id,
            message_ts,
            &status_line,
            Some(&result_blocks),
        )
        .await
        {
            log_slack_transport_event(SlackTransportEvent {
                operation: "approval_update_message",
                outcome: "error",
                channel_id,
                message_id: message_ts,
                user_id: &reviewer_id,
                command: action_type,
                webhook_port: None,
                message_len: None,
                block_count: Some(result_blocks.len()),
                error: Some(&e),
            });
        }
    }
}

/// Handle a Slack slash command (`/plan`, `/execute`).
///
/// Dispatches `Channel.ReceiveMessage` with the `command` field set so
/// `route_message` WASM can spawn or switch the session to the requested mode.
async fn handle_slash_command(
    payload: serde_json::Value,
    channel_entity_id: &Arc<tokio::sync::RwLock<Option<String>>>,
    api: &crate::PawApiClient,
) {
    let raw_command = payload
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let channel_id = payload
        .get("channel_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let trigger_id = payload
        .get("trigger_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Strip leading "/" from command name
    let command = raw_command.strip_prefix('/').unwrap_or(raw_command);
    if command != "plan" && command != "execute" {
        log_slack_transport_event(SlackTransportEvent {
            operation: "slash_command",
            outcome: "ignored",
            channel_id,
            message_id: trigger_id,
            user_id,
            command: raw_command,
            webhook_port: None,
            message_len: Some(text.len()),
            block_count: None,
            error: None,
        });
        return;
    }

    let entity_id = channel_entity_id.read().await.clone();
    let Some(entity_id) = entity_id else {
        log_slack_transport_event(SlackTransportEvent {
            operation: "slash_command",
            outcome: "error",
            channel_id,
            message_id: trigger_id,
            user_id,
            command,
            webhook_port: None,
            message_len: Some(text.len()),
            block_count: None,
            error: Some("no channel entity bootstrapped"),
        });
        return;
    };

    let params = serde_json::json!({
        "message_id": trigger_id,
        "author_id": user_id,
        "thread_id": channel_id,
        "content": text,
        "command": command,
    });

    match api
        .dispatch_action("Channels", &entity_id, "Paw.Channel.ReceiveMessage", params)
        .await
    {
        Ok(_) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "slash_command",
                outcome: "success",
                channel_id,
                message_id: trigger_id,
                user_id,
                command,
                webhook_port: None,
                message_len: Some(text.len()),
                block_count: None,
                error: None,
            });
        }
        Err(e) => {
            log_slack_transport_event(SlackTransportEvent {
                operation: "slash_command",
                outcome: "error",
                channel_id,
                message_id: trigger_id,
                user_id,
                command,
                webhook_port: None,
                message_len: Some(text.len()),
                block_count: None,
                error: Some(&e),
            });
        }
    }
}
