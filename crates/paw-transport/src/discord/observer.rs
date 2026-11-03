//! Discord observer — subscribes to Temper SSE event stream and forwards
//! formatted entity state changes to Discord guild channels.
//!
//! - **#feed channel**: compact one-line embeds for every meaningful transition.
//! - **agent-work forum**: one forum post per WorkCycle with gate-tracking top embed.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use serde::Deserialize;

use super::gateway::{
    DISCORD_API_BASE, create_forum_post, edit_message, send_channel_message, send_thread_message,
};
use super::types::{Embed, EmbedField, EmbedFooter, embed_colors};
use crate::PawApiClient;

/// Configuration for the Discord observer.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Discord bot token.
    pub bot_token: String,
    /// Text channel ID for the #feed stream (compact one-liners).
    pub feed_channel_id: Option<String>,
    /// Forum channel ID for per-WorkCycle threads.
    pub forum_channel_id: Option<String>,
}

/// SSE state-change event payload from `/observe/events/stream`.
#[derive(Debug, Clone, Deserialize)]
struct StateChangeEvent {
    #[allow(dead_code)]
    seq: u64,
    entity_type: String,
    entity_id: String,
    action: String,
    status: String,
    #[allow(dead_code)]
    tenant: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    session_id: Option<String>,
}

/// Tracks a forum post created for a WorkCycle.
struct ForumPostState {
    thread_id: String,
    top_message_id: String,
}

/// Entity types we care about.
const OBSERVED_TYPES: &[&str] = &["Agent", "WorkCycle", "Issue"];

/// Run the Discord observer loop.
///
/// Subscribes to the Temper SSE event stream, parses `state_change` events,
/// and forwards formatted embeds to Discord channels. Reconnects on stream
/// drop with a 5-second backoff.
pub async fn run_observer(api: PawApiClient, config: ObserverConfig) -> Result<(), String> {
    if config.feed_channel_id.is_none() && config.forum_channel_id.is_none() {
        tracing::warn!("Discord observer: no feed or forum channel configured, exiting");
        return Ok(());
    }

    let http = reqwest::Client::new();
    let mut forum_posts: BTreeMap<String, ForumPostState> = BTreeMap::new();
    let mut soul_cache: BTreeMap<String, String> = BTreeMap::new();

    loop {
        tracing::info!("Discord observer: connecting to SSE stream...");
        match connect_and_process(&api, &config, &http, &mut forum_posts, &mut soul_cache).await {
            Ok(()) => {
                tracing::info!("Discord observer: SSE stream ended cleanly, reconnecting...");
            }
            Err(e) => {
                tracing::error!("Discord observer: stream error: {e}, reconnecting in 5s...");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Connect to the SSE stream and process events until the stream ends or errors.
async fn connect_and_process(
    api: &PawApiClient,
    config: &ObserverConfig,
    http: &reqwest::Client,
    forum_posts: &mut BTreeMap<String, ForumPostState>,
    soul_cache: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let resp = api.subscribe_events().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("SSE endpoint returned {status}"));
    }

    tracing::info!("Discord observer: connected to SSE stream");

    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();
    let mut current_event_type = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("stream read error: {e}"))?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);

        // Process complete lines from the buffer.
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                // Empty line = end of event. Reset for next event.
                current_event_type.clear();
                continue;
            }

            if let Some(event_name) = line.strip_prefix("event: ") {
                current_event_type = event_name.trim().to_string();
                continue;
            }

            if let Some(data) = line.strip_prefix("data: ") {
                if current_event_type != "state_change" {
                    continue;
                }

                let event: StateChangeEvent = match serde_json::from_str(data) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::debug!("Discord observer: failed to parse SSE data: {e}");
                        continue;
                    }
                };

                if !OBSERVED_TYPES.contains(&event.entity_type.as_str()) {
                    continue;
                }

                tracing::debug!(
                    entity_type = %event.entity_type,
                    entity_id = %event.entity_id,
                    status = %event.status,
                    "Discord observer: processing event"
                );

                // Spawn a task for each event so slow Discord calls don't block.
                let config_clone = config.clone();
                let http_clone = http.clone();
                let event_clone = event.clone();
                let api_clone = api.clone();

                // For feed channel: fire-and-forget in a spawned task.
                if let Some(ref feed_id) = config_clone.feed_channel_id {
                    let feed_id = feed_id.clone();
                    let http_c = http_clone.clone();
                    let bot_token = config_clone.bot_token.clone();
                    let evt = event_clone.clone();
                    let soul_name =
                        resolve_soul_name(&api_clone, soul_cache, evt.agent_id.as_deref()).await;

                    let embed = build_feed_embed(&evt, soul_name.as_deref());
                    tokio::spawn(async move {
                        if let Err(e) =
                            send_channel_message(&http_c, &bot_token, &feed_id, "", vec![embed])
                                .await
                        {
                            tracing::error!("Discord observer: feed message failed: {e}");
                        }
                    });
                }

                // For forum channel: handle WorkCycle events inline to track state.
                if config.forum_channel_id.is_some() && event_clone.entity_type == "WorkCycle" {
                    handle_work_cycle_event(&event_clone, config, http, &api_clone, forum_posts)
                        .await;
                }
            }
        }
    }

    Ok(())
}

/// Build a compact one-line embed for the #feed channel.
fn build_feed_embed(event: &StateChangeEvent, soul_name: Option<&str>) -> Embed {
    let color = status_color(&event.status);
    let short_id = short_entity_id(&event.entity_id);

    let description = if let Some(name) = soul_name {
        format!(
            "**{}** `{}`: {} \u{2192} {}",
            event.entity_type, name, event.action, event.status
        )
    } else {
        format!(
            "**{}** `{}`: {} \u{2192} {}",
            event.entity_type, short_id, event.action, event.status
        )
    };

    let timestamp = chrono_now_short();

    Embed {
        title: None,
        description: Some(description),
        color: Some(color),
        fields: None,
        footer: Some(EmbedFooter {
            text: format!("{} {} \u{2022} {}", event.entity_type, short_id, timestamp),
        }),
    }
}

/// Handle a WorkCycle event for the forum channel.
async fn handle_work_cycle_event(
    event: &StateChangeEvent,
    config: &ObserverConfig,
    http: &reqwest::Client,
    api: &PawApiClient,
    forum_posts: &mut BTreeMap<String, ForumPostState>,
) {
    let forum_id = match config.forum_channel_id.as_deref() {
        Some(id) => id,
        None => return,
    };

    let is_created = event.status == "Created";
    let is_terminal = matches!(event.status.as_str(), "Complete" | "Failed");
    let is_gate_change = matches!(
        event.action.as_str(),
        "PassPlanGate" | "PassTestGate" | "PassReviewGate" | "MarkPlanReady" | "MarkTestsPassed"
    );

    if is_created {
        // Fetch the entity to get task_summary.
        let summary = fetch_work_cycle_summary(api, &event.entity_id).await;
        let thread_name = summary.as_deref().unwrap_or(&event.entity_id);
        // Truncate to 100 chars for Discord thread name limit.
        let thread_name = if thread_name.len() > 100 {
            &thread_name[..thread_name.floor_char_boundary(100)]
        } else {
            thread_name
        };

        let gates = fetch_work_cycle_gates(api, &event.entity_id).await;
        let top_embed = build_gates_embed(&event.entity_id, &gates, &event.status);

        match create_forum_post(
            http,
            &config.bot_token,
            forum_id,
            thread_name,
            "",
            vec![top_embed],
            vec![],
        )
        .await
        {
            Ok(resp) => {
                // The forum post response contains the thread ID.
                // The initial message ID is in resp.message.id if available,
                // but ForumThreadResponse only has `id` (thread ID).
                // We need to fetch the first message from the thread.
                let thread_id = resp.id.clone();
                let top_msg_id = fetch_first_message_id(http, &config.bot_token, &thread_id).await;
                forum_posts.insert(
                    event.entity_id.clone(),
                    ForumPostState {
                        thread_id,
                        top_message_id: top_msg_id,
                    },
                );
                tracing::info!(
                    entity_id = %event.entity_id,
                    "Discord observer: created forum post for WorkCycle"
                );
            }
            Err(e) => {
                tracing::error!("Discord observer: failed to create forum post: {e}");
            }
        }
        return;
    }

    // For subsequent events, we need the existing forum post state.
    let post = match forum_posts.get(&event.entity_id) {
        Some(p) => p,
        None => {
            tracing::debug!(
                entity_id = %event.entity_id,
                "Discord observer: no forum post tracked for WorkCycle, skipping"
            );
            return;
        }
    };

    // Gate changes: update the top embed.
    if is_gate_change || is_terminal {
        let gates = fetch_work_cycle_gates(api, &event.entity_id).await;
        let color_status = if is_terminal {
            &event.status
        } else {
            // Keep the current status for non-terminal gate updates.
            "InProgress"
        };
        let updated_embed = build_gates_embed(&event.entity_id, &gates, color_status);

        if let Err(e) = edit_message(
            http,
            &config.bot_token,
            &post.thread_id,
            &post.top_message_id,
            None,
            Some(vec![updated_embed]),
        )
        .await
        {
            tracing::error!("Discord observer: failed to edit forum top embed: {e}");
        }
    }

    // Send a thread reply for every transition.
    let reply_embed = Embed {
        title: None,
        description: Some(format!("{} \u{2192} **{}**", event.action, event.status)),
        color: Some(status_color(&event.status)),
        fields: None,
        footer: Some(EmbedFooter {
            text: chrono_now_short(),
        }),
    };

    if let Err(e) = send_thread_message(
        http,
        &config.bot_token,
        &post.thread_id,
        "",
        vec![reply_embed],
    )
    .await
    {
        tracing::error!("Discord observer: failed to send thread reply: {e}");
    }
}

/// Build the gates-tracking embed for the top of a WorkCycle forum post.
fn build_gates_embed(entity_id: &str, gates: &WorkCycleGates, status: &str) -> Embed {
    let check = "\u{2705}"; // green check
    let pending = "\u{2b1c}"; // white square

    let fields = vec![
        EmbedField {
            name: "Plan".to_string(),
            value: if gates.has_plan {
                check.to_string()
            } else {
                pending.to_string()
            },
            inline: true,
        },
        EmbedField {
            name: "Tests".to_string(),
            value: if gates.tests_passed {
                check.to_string()
            } else {
                pending.to_string()
            },
            inline: true,
        },
        EmbedField {
            name: "Review".to_string(),
            value: if gates.review_passed {
                check.to_string()
            } else {
                pending.to_string()
            },
            inline: true,
        },
        EmbedField {
            name: "Status".to_string(),
            value: status.to_string(),
            inline: false,
        },
    ];

    Embed {
        title: Some(format!("WorkCycle {}", short_entity_id(entity_id))),
        description: None,
        color: Some(status_color(status)),
        fields: Some(fields),
        footer: Some(EmbedFooter {
            text: format!("{} \u{2022} {}", entity_id, chrono_now_short()),
        }),
    }
}

/// Gate values for a WorkCycle entity.
#[derive(Default)]
struct WorkCycleGates {
    has_plan: bool,
    tests_passed: bool,
    review_passed: bool,
}

/// Fetch WorkCycle gate fields via OData.
async fn fetch_work_cycle_gates(api: &PawApiClient, entity_id: &str) -> WorkCycleGates {
    match api.get_entity("WorkCycles", entity_id).await {
        Ok(val) => WorkCycleGates {
            has_plan: val
                .get("has_plan")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            tests_passed: val
                .get("tests_passed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            review_passed: val
                .get("review_passed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        },
        Err(e) => {
            tracing::debug!("Discord observer: failed to fetch WorkCycle gates: {e}");
            WorkCycleGates::default()
        }
    }
}

/// Fetch the task_summary field from a WorkCycle entity.
async fn fetch_work_cycle_summary(api: &PawApiClient, entity_id: &str) -> Option<String> {
    match api.get_entity("WorkCycles", entity_id).await {
        Ok(val) => val
            .get("task_summary")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        Err(_) => None,
    }
}

/// Fetch the first message ID from a thread (used to get the top embed message).
async fn fetch_first_message_id(
    http: &reqwest::Client,
    bot_token: &str,
    thread_id: &str,
) -> String {
    let url = format!("{DISCORD_API_BASE}/channels/{thread_id}/messages?limit=1&sort_order=asc");
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bot {bot_token}"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            if let Ok(messages) = r.json::<Vec<serde_json::Value>>().await
                && let Some(msg) = messages.first()
                && let Some(id) = msg.get("id").and_then(|v| v.as_str())
            {
                return id.to_string();
            }
            // Fallback: use thread_id as message_id (forum posts share the same ID).
            thread_id.to_string()
        }
        _ => thread_id.to_string(),
    }
}

/// Resolve a soul name from cache or fetch via OData.
async fn resolve_soul_name(
    api: &PawApiClient,
    cache: &mut BTreeMap<String, String>,
    agent_id: Option<&str>,
) -> Option<String> {
    let agent_id = agent_id?;
    if agent_id.is_empty() {
        return None;
    }

    if let Some(name) = cache.get(agent_id) {
        return Some(name.clone());
    }

    // Try to fetch the agent's soul_id, then get the soul name.
    if let Ok(agent) = api.get_entity("Agents", agent_id).await {
        if let Some(soul_id) = agent.get("soul_id").and_then(|v| v.as_str())
            && let Ok(soul) = api.get_entity("Souls", soul_id).await
            && let Some(name) = soul.get("name").and_then(|v| v.as_str())
        {
            let name = name.to_string();
            cache.insert(agent_id.to_string(), name.clone());
            return Some(name);
        }
        // Fallback: use the agent's own name if available.
        if let Some(name) = agent.get("name").and_then(|v| v.as_str()) {
            let name = name.to_string();
            cache.insert(agent_id.to_string(), name.clone());
            return Some(name);
        }
    }

    None
}

/// Map a status string to a Discord embed color.
fn status_color(status: &str) -> u32 {
    match status {
        // Green — success / complete
        "Completed" | "Complete" | "Resolved" | "Done" | "AlertResolved" => embed_colors::SUCCESS,
        // Red — error / failure
        "Failed" | "Cancelled" | "Escalate" | "AlertPersists" => embed_colors::ERROR,
        // Blurple — active / in-progress
        "Created" | "Thinking" | "Executing" | "InProgress" | "Triaging" | "Planning" => {
            embed_colors::INFO
        }
        // Yellow — waiting / review
        "WaitingForApproval" | "Reviewing" | "Steering" | "Testing" => embed_colors::WARNING,
        // Grey — idle / paused / default
        "Idle" | "Paused" => embed_colors::IDLE,
        _ => embed_colors::IDLE,
    }
}

/// Shorten an entity ID for display (e.g., "ag-7f3a1b2c" → "ag-7f3a").
fn short_entity_id(id: &str) -> &str {
    if id.len() <= 8 {
        id
    } else {
        // Keep prefix up to 8 chars.
        &id[..id.floor_char_boundary(8)]
    }
}

/// Current timestamp as a short string for embed footers.
fn chrono_now_short() -> String {
    // Use a simple UTC timestamp without pulling in the chrono crate.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Format as HH:MM:SS UTC.
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}
