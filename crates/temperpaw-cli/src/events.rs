use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use paw_transport::PawApiClient;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};

#[derive(Debug, Clone)]
pub enum TuiEvent {
    Reply { content: String },
    Status { text: String },
    System { text: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StateChangeEvent {
    pub entity_type: String,
    pub entity_id: String,
    pub action: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyEvent {
    pub key: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeEvent {
    pub key: String,
    pub content: String,
}

pub async fn watch_cli_channel(
    api: PawApiClient,
    channel_entity_id: String,
    active_thread: Arc<Mutex<String>>,
    tx: mpsc::UnboundedSender<TuiEvent>,
) {
    let mut seen = HashSet::new();
    if let Ok(channel) = api.get_entity("Channels", &channel_entity_id).await {
        mark_existing_replies(&channel, &mut seen);
    }

    let mut poll = interval(Duration::from_secs(2));
    let mut stream_buffer = String::new();
    let mut event_stream = match api.subscribe_events().await {
        Ok(resp) if resp.status().is_success() => Some(resp.bytes_stream()),
        _ => None,
    };

    loop {
        tokio::select! {
            _ = poll.tick() => {
                fetch_and_emit_replies(&api, &channel_entity_id, &active_thread, &mut seen, &tx).await;
                fetch_and_emit_review_context(&api, &channel_entity_id, &active_thread, &mut seen, &tx).await;
            }
            maybe_chunk = async {
                match event_stream.as_mut() {
                    Some(stream) => stream.next().await,
                    None => std::future::pending().await,
                }
            } => {
                match maybe_chunk {
                    Some(Ok(chunk)) => {
                        let text = String::from_utf8_lossy(&chunk);
                        stream_buffer.push_str(&text);
                        for event in drain_state_change_events(&mut stream_buffer) {
                            if event.entity_type == "Channel" && event.entity_id == channel_entity_id {
                                let _ = tx.send(TuiEvent::Status { text: event.status });
                                fetch_and_emit_replies(&api, &channel_entity_id, &active_thread, &mut seen, &tx).await;
                                fetch_and_emit_review_context(&api, &channel_entity_id, &active_thread, &mut seen, &tx).await;
                            } else if matches!(event.entity_type.as_str(), "Session" | "Plan" | "GovernanceDecision") {
                                fetch_and_emit_review_context(&api, &channel_entity_id, &active_thread, &mut seen, &tx).await;
                            }
                        }
                    }
                    Some(Err(_error)) => {
                        event_stream = None;
                    }
                    None => {
                        event_stream = None;
                    }
                }
            }
        }
    }
}

async fn fetch_and_emit_review_context(
    api: &PawApiClient,
    channel_entity_id: &str,
    active_thread: &Arc<Mutex<String>>,
    seen: &mut HashSet<String>,
    tx: &mpsc::UnboundedSender<TuiEvent>,
) {
    let thread_id = active_thread
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| "main".to_string());

    let Ok(channel) = api.get_entity("Channels", channel_entity_id).await else {
        return;
    };
    for notice in extract_channel_failures(&channel, &thread_id) {
        emit_notice(notice, seen, tx);
    }

    let channel_id = nested_field_str(&channel, &["channel_id", "ChannelId"]).unwrap_or("");
    let active_session_id = if channel_id.is_empty() {
        None
    } else {
        find_active_channel_session(api, channel_id, &thread_id)
            .await
            .and_then(|session| {
                nested_field_str(&session, &["session_entity_id", "SessionEntityId"])
                    .map(str::to_string)
            })
    };

    if let Some(session_id) = active_session_id.as_deref()
        && let Ok(session) = api.get_entity("Sessions", session_id).await
    {
        if let Some(status) = entity_status(&session) {
            let _ = tx.send(TuiEvent::Status {
                text: session_status_label(status),
            });
        }
        for notice in extract_session_alerts(&session) {
            emit_notice(notice, seen, tx);
        }
    }

    let decisions_url = format!(
        "{}/api/tenants/{}/decisions?status=pending",
        api.config().base_url,
        api.config().tenant
    );
    if let Ok(decisions) = api.raw_get(&decisions_url).await {
        for notice in extract_decision_items(&decisions, active_session_id.as_deref()) {
            emit_notice(notice, seen, tx);
        }
    }

    let plans_url = format!("{}/tdata/Plans", api.config().base_url);
    if let Ok(plans_response) = api.raw_get(&plans_url).await {
        let plans = plans_response
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for notice in extract_plan_review_items(&plans, active_session_id.as_deref()) {
            emit_notice(notice, seen, tx);
        }
    }
}

fn emit_notice(
    notice: NoticeEvent,
    seen: &mut HashSet<String>,
    tx: &mpsc::UnboundedSender<TuiEvent>,
) {
    if seen.insert(notice.key) {
        let _ = tx.send(TuiEvent::System {
            text: notice.content,
        });
    }
}

async fn fetch_and_emit_replies(
    api: &PawApiClient,
    channel_entity_id: &str,
    active_thread: &Arc<Mutex<String>>,
    seen: &mut HashSet<String>,
    tx: &mpsc::UnboundedSender<TuiEvent>,
) {
    let thread_id = active_thread
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| "main".to_string());

    let Ok(channel) = api.get_entity("Channels", channel_entity_id).await else {
        return;
    };
    for reply in extract_reply_events(&channel, &thread_id) {
        if seen.insert(reply.key) {
            let _ = tx.send(TuiEvent::Reply {
                content: reply.content,
            });
            let _ = tx.send(TuiEvent::Status {
                text: "idle".to_string(),
            });
        }
    }
}

async fn find_active_channel_session(
    api: &PawApiClient,
    channel_id: &str,
    thread_id: &str,
) -> Option<Value> {
    let escaped_thread = escape_odata(thread_id);
    let sessions = api
        .query_entities(
            "ChannelSessions",
            &format!("thread_id eq '{escaped_thread}'"),
            20,
        )
        .await
        .ok()?;
    select_active_channel_session(sessions, channel_id, thread_id)
}

fn select_active_channel_session(
    sessions: Vec<Value>,
    channel_id: &str,
    thread_id: &str,
) -> Option<Value> {
    sessions.into_iter().find(|session| {
        entity_status(session) == Some("Active")
            && nested_field_str(session, &["channel_id", "ChannelId"]) == Some(channel_id)
            && nested_field_str(session, &["thread_id", "ThreadId"]) == Some(thread_id)
    })
}

fn mark_existing_replies(entity: &Value, seen: &mut HashSet<String>) {
    for event in entity_events(entity) {
        if matches!(event_action(event), Some("ReplyDelivered")) {
            seen.insert(event_key(event, ""));
        }
    }
}

pub fn extract_reply_events(entity: &Value, thread_id: &str) -> Vec<ReplyEvent> {
    entity_events(entity)
        .into_iter()
        .filter_map(|event| {
            let action = event_action(event)?;
            if action != "ReplyDelivered" {
                return None;
            }
            let params = event_params(event)?;
            let event_thread = field_str(params, &["thread_id", "ThreadId"])?;
            if event_thread != thread_id {
                return None;
            }
            let content = field_str(params, &["content", "Content"])?;
            if content.trim().is_empty() {
                return None;
            }
            Some(ReplyEvent {
                key: event_key(event, thread_id),
                content: content.to_string(),
            })
        })
        .collect()
}

pub fn extract_session_alerts(session: &Value) -> Vec<NoticeEvent> {
    let session_id = entity_id(session).unwrap_or("unknown-session");
    let status = entity_status(session).unwrap_or("");
    let mut notices = Vec::new();

    if status == "WaitingForApproval" {
        if let Some(decision_id) =
            nested_field_str(session, &["pending_decision_id", "PendingDecisionId"])
                .filter(|value| !value.trim().is_empty())
        {
            let method = nested_field_str(session, &["pending_tool_context", "PendingToolContext"])
                .and_then(tool_context_method)
                .unwrap_or("unknown action".to_string());
            notices.push(NoticeEvent {
                key: format!("session:{session_id}:decision:{decision_id}"),
                content: format!(
                    "Permission Required\nSession `{session_id}` is waiting to run `{method}`.\nDecision: `{decision_id}`\nUse `/approve-always {decision_id}`, `/approve-session {decision_id}`, `/approve-once {decision_id}`, or `/deny {decision_id}`."
                ),
            });
        }

        if let Some(plan_id) = nested_field_str(session, &["active_plan_id", "ActivePlanId"])
            .filter(|value| !value.trim().is_empty())
        {
            notices.push(NoticeEvent {
                key: format!("session:{session_id}:plan:{plan_id}"),
                content: format!(
                    "Plan Review Required\nSession `{session_id}` is waiting on plan `{plan_id}`.\nUse `/plan-approve {plan_id}` or `/request-changes {plan_id} <notes>`."
                ),
            });
        }
    }

    if matches!(status, "Failed" | "Cancelled") {
        let error = nested_field_str(
            session,
            &["error_message", "ErrorMessage", "result", "Result"],
        )
        .unwrap_or("no error message recorded");
        let verb = if status == "Failed" {
            "failed"
        } else {
            "was cancelled"
        };
        notices.push(NoticeEvent {
            key: format!("session:{session_id}:{status}:{error}"),
            content: format!("Session `{session_id}` {verb}: {error}"),
        });
    }

    notices
}

pub fn extract_decision_items(
    response: &Value,
    active_session_id: Option<&str>,
) -> Vec<NoticeEvent> {
    response
        .get("decisions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|decision| field_str(decision, &["status"]) == Some("pending"))
        .filter(|decision| {
            active_session_id.is_none_or(|active| {
                field_str(decision, &["session_id"])
                    .filter(|session_id| !session_id.trim().is_empty())
                    .is_none_or(|session_id| session_id == active)
            })
        })
        .filter_map(|decision| {
            let decision_id = field_str(decision, &["id"])?;
            let action = field_str(decision, &["action"]).unwrap_or("unknown action");
            let resource_type = field_str(decision, &["resource_type"]).unwrap_or("resource");
            let resource_id = field_str(decision, &["resource_id"]).unwrap_or("");
            let reason = field_str(decision, &["denial_reason"]).unwrap_or("");
            Some(NoticeEvent {
                key: format!("decision:{decision_id}"),
                content: format!(
                    "Permission Required\nAction: `{action}`\nResource: `{resource_type}:{resource_id}`\nReason: {reason}\nDecision: `{decision_id}`\nUse `/approve-always {decision_id}`, `/approve-session {decision_id}`, `/approve-once {decision_id}`, or `/deny {decision_id}`."
                ),
            })
        })
        .collect()
}

pub fn extract_plan_review_items(
    plans: &[Value],
    active_session_id: Option<&str>,
) -> Vec<NoticeEvent> {
    plans
        .iter()
        .filter(|plan| matches!(entity_status(plan), Some("UnderReview" | "Escalated")))
        .filter(|plan| {
            active_session_id.is_none_or(|active| {
                nested_field_str(plan, &["session_id", "SessionId"])
                    .filter(|session_id| !session_id.trim().is_empty())
                    .is_none_or(|session_id| session_id == active)
            })
        })
        .filter_map(|plan| {
            let plan_id = entity_id(plan)?;
            let status = entity_status(plan).unwrap_or("UnderReview");
            let description = nested_field_str(plan, &["description", "Description"]).unwrap_or("");
            let plan_text = nested_field_str(plan, &["plan_text", "PlanText"]).unwrap_or("");
            let snippet = truncate_text(plan_text, 360);
            let mut lines = vec![
                "Plan Review Required".to_string(),
                format!("Plan: `{plan_id}` ({status})"),
            ];
            if !description.is_empty() {
                lines.push(format!("Summary: {description}"));
            }
            if !snippet.is_empty() {
                lines.push(format!("Excerpt:\n{snippet}"));
            }
            lines.push(format!(
                "Use `/plan-approve {plan_id}` or `/request-changes {plan_id} <notes>`."
            ));
            Some(NoticeEvent {
                key: format!("plan:{plan_id}:{status}"),
                content: lines.join("\n"),
            })
        })
        .collect()
}

fn extract_channel_failures(entity: &Value, thread_id: &str) -> Vec<NoticeEvent> {
    entity_events(entity)
        .into_iter()
        .filter_map(|event| {
            let action = event_action(event)?;
            if !matches!(action, "RouteFailed" | "ReplyFailed" | "ConnectFailed") {
                return None;
            }
            let params = event_params(event).unwrap_or(&Value::Null);
            if let Some(event_thread) = field_str(params, &["thread_id", "ThreadId"])
                && event_thread != thread_id
            {
                return None;
            }
            let error = field_str(params, &["error", "error_message", "ErrorMessage"])
                .or_else(|| field_str(entity, &["error", "error_message", "ErrorMessage"]))
                .unwrap_or("no error message recorded");
            let key = event_key(event, thread_id);
            Some(NoticeEvent {
                key: format!("failure:{key}"),
                content: format!("{action}: {error}"),
            })
        })
        .collect()
}

pub fn drain_state_change_events(buffer: &mut String) -> Vec<StateChangeEvent> {
    let mut events = Vec::new();
    while let Some(idx) = buffer.find("\n\n") {
        let frame = buffer[..idx].to_string();
        buffer.drain(..idx + 2);
        if let Some(event) = parse_sse_frame(&frame) {
            events.push(event);
        }
    }
    events
}

fn parse_sse_frame(frame: &str) -> Option<StateChangeEvent> {
    let mut event_name = "";
    let mut data = String::new();

    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim());
        }
    }

    if event_name != "state_change" || data.is_empty() {
        return None;
    }

    serde_json::from_str(&data).ok()
}

fn entity_events(entity: &Value) -> Vec<&Value> {
    entity
        .get("_events")
        .or_else(|| entity.get("events"))
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn event_action(event: &Value) -> Option<&str> {
    event
        .get("action")
        .or_else(|| event.get("Action"))
        .and_then(Value::as_str)
}

fn event_params(event: &Value) -> Option<&Value> {
    event.get("params").or_else(|| event.get("Params"))
}

fn field_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn nested_field_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    field_str(value, keys).or_else(|| {
        value
            .get("fields")
            .and_then(|fields| field_str(fields, keys))
    })
}

fn entity_id(value: &Value) -> Option<&str> {
    nested_field_str(value, &["entity_id", "Id", "id"])
}

fn entity_status(value: &Value) -> Option<&str> {
    nested_field_str(value, &["status", "Status"])
}

fn tool_context_method(raw: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    parsed
        .get("tool_context")
        .unwrap_or(&parsed)
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn session_status_label(status: &str) -> String {
    match status {
        "WaitingForApproval" => "waiting for approval".to_string(),
        "CallingProvider" => "calling provider".to_string(),
        "Executing" => "executing tools".to_string(),
        "PreparingContext" => "preparing context".to_string(),
        "Compacting" => "compacting context".to_string(),
        "Completed" => "idle".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

fn truncate_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn escape_odata(value: &str) -> String {
    value.replace('\'', "''")
}

fn event_key(event: &Value, thread_id: &str) -> String {
    let action = event_action(event).unwrap_or("");
    let event_thread = event_params(event)
        .and_then(|params| field_str(params, &["thread_id", "ThreadId"]))
        .unwrap_or("");
    let thread_id = if thread_id.is_empty() {
        event_thread
    } else {
        thread_id
    };
    let timestamp = event
        .get("timestamp")
        .or_else(|| event.get("Timestamp"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let content = event_params(event)
        .and_then(|params| field_str(params, &["content", "Content"]))
        .unwrap_or("");
    format!("{thread_id}:{action}:{timestamp}:{content}")
}
