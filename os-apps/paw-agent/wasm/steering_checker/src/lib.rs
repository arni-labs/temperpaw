//! Steering Checker — WASM module for the two-loop steering architecture.
//!
//! When the LLM returns end_turn, this module is triggered (via CheckSteering).
//! It checks for queued steering messages and either:
//! - Injects the first queued message and returns ContinueWithSteering
//! - Returns FinalizeResult or FinalizeResultNoReply if no messages are queued
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use session_tree_lib::SessionTree;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    create_content_file_ref, is_session_entries_ref, read_content_file, read_content_file_version,
    read_session_from_temperfs, resolve_temper_api_url, write_session_to_temperfs,
};

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        ctx.log("info", "steering_checker: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        // Read steering state
        let steering_messages_json = fields
            .get("steering_messages")
            .and_then(|v| v.as_str())
            .unwrap_or("[]");

        let mut steering_messages: Vec<Value> =
            serde_json::from_str(steering_messages_json).unwrap_or_default();

        let follow_up_count = fields
            .get("follow_up_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let max_follow_ups: i64 = fields
            .get("max_follow_ups")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let session_file_id = fields
            .get("session_file_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let session_leaf_id = fields
            .get("session_leaf_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let tenant = &ctx.tenant;

        // Check if we have steering messages AND haven't hit the follow-up limit
        if !steering_messages.is_empty() && follow_up_count < max_follow_ups {
            // Dequeue the first steering message
            let msg = steering_messages.remove(0);
            let msg_content = msg
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| msg.as_str().unwrap_or(""));

            ctx.log(
                "info",
                &format!(
                    "steering_checker: injecting steering message ({} remaining, follow_up {}/{})",
                    steering_messages.len(),
                    follow_up_count + 1,
                    max_follow_ups
                ),
            );

            // If session tree mode, inject into session tree
            if !session_file_id.is_empty() && !session_leaf_id.is_empty() {
                let session_jsonl = read_session_from_temperfs(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    &fields,
                    session_file_id,
                )?;
                let mut tree = SessionTree::from_jsonl(&session_jsonl);
                let workspace_id = fields
                    .get("workspace_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let entry_id = format!("s-{}", tree.len());
                let entity_backed_session = is_session_entries_ref(session_file_id);
                let new_leaf_id = if !entity_backed_session && !workspace_id.is_empty() {
                    match create_content_file_ref(
                        &ctx,
                        &temper_api_url,
                        tenant,
                        workspace_id,
                        &format!("msg-{entry_id}.txt"),
                        msg_content,
                    ) {
                        Ok(content_ref) => {
                            let _line = tree.append_entry_with_file(
                                &entry_id,
                                Some(session_leaf_id),
                                session_tree_lib::EntryType::Steering,
                                Some("user"),
                                &content_ref.file_id,
                                Some(&content_ref.file_version_id),
                                estimate_tokens(msg_content),
                                None,
                            );
                            entry_id.clone()
                        }
                        Err(_) => {
                            let (leaf, _) = tree.append_steering_message(
                                session_leaf_id,
                                msg_content,
                                estimate_tokens(msg_content),
                            );
                            leaf
                        }
                    }
                } else {
                    let (leaf, _) = tree.append_steering_message(
                        session_leaf_id,
                        msg_content,
                        estimate_tokens(msg_content),
                    );
                    leaf
                };

                // Write back
                let updated_jsonl = tree.to_jsonl();
                write_session_to_temperfs(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    &fields,
                    session_file_id,
                    &updated_jsonl,
                )?;

                // Update steering_messages in entity state (remove dequeued message)
                let updated_queue =
                    serde_json::to_string(&steering_messages).unwrap_or_else(|_| "[]".to_string());
                set_success_result(
                    "ContinueWithSteering",
                    &json!({
                        "session_leaf_id": new_leaf_id,
                        "steering_messages": updated_queue,
                    }),
                );
            } else {
                // Inline conversation mode (legacy fallback)
                let conversation_json = fields
                    .get("conversation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("[]");
                let mut messages: Vec<Value> =
                    serde_json::from_str(conversation_json).unwrap_or_default();
                messages.push(json!({
                    "role": "user",
                    "content": msg_content,
                }));
                let updated_conversation = serde_json::to_string(&messages).unwrap_or_default();

                set_success_result(
                    "ContinueWithSteering",
                    &json!({
                        "conversation": updated_conversation,
                        "steering_messages": serde_json::to_string(&steering_messages)
                            .unwrap_or_else(|_| "[]".to_string()),
                    }),
                );
            }
        } else {
            // No steering messages or follow-up limit reached — finalize
            if follow_up_count >= max_follow_ups {
                ctx.log(
                    "info",
                    &format!(
                        "steering_checker: follow-up limit reached ({}/{}), finalizing",
                        follow_up_count, max_follow_ups
                    ),
                );
            } else {
                ctx.log("info", "steering_checker: no steering messages, finalizing");
            }

            // Extract the result text from the last assistant message
            let result_text = extract_last_result(
                &ctx,
                &fields,
                &temper_api_url,
                tenant,
                session_file_id,
                session_leaf_id,
            )?;

            let terminal_action = if should_finalize_without_reply(&ctx.entity_id, &fields) {
                "FinalizeResultNoReply"
            } else {
                "FinalizeResult"
            };

            // Track final result dispatches — paired with temperpaw#60 fix.
            // Before Phase 2a + 2b landed, the callback for this action could be
            // dropped under heartbeat contention. This log line lets Datadog
            // confirm the dispatch actually reached the server.
            ctx.log("info", &format!(
                "steering_checker: dispatching {terminal_action} session_id={} follow_up_count={} result_len={}",
                ctx.entity_id, follow_up_count, result_text.len()
            ));

            set_success_result(
                terminal_action,
                &json!({
                    "result": result_text,
                    "session_leaf_id": session_leaf_id,
                    "pending_tool_calls": "",
                    "pending_tool_context": "",
                    "pending_decision_id": "",
                    "reply_attachments_json": string_field(&fields, &["reply_attachments_json", "ReplyAttachmentsJson"])
                        .unwrap_or(""),
                }),
            );
        }

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

/// Extract the last assistant text from the conversation for the result field.
fn extract_last_result(
    ctx: &Context,
    fields: &Value,
    temper_api_url: &str,
    tenant: &str,
    session_file_id: &str,
    session_leaf_id: &str,
) -> Result<String, String> {
    if !session_file_id.is_empty() && !session_leaf_id.is_empty() {
        let session_jsonl =
            read_session_from_temperfs(ctx, temper_api_url, tenant, fields, session_file_id)?;
        let tree = SessionTree::from_jsonl(&session_jsonl);
        let refs = tree.build_context_refs(session_leaf_id);

        for msg_ref in refs.iter().rev() {
            if msg_ref.role != "assistant" {
                continue;
            }
            if let Some(ref file_version_id) = msg_ref.content_file_version_id {
                match read_content_file_version(
                    ctx,
                    temper_api_url,
                    tenant,
                    fields,
                    file_version_id,
                ) {
                    Ok(raw) if !raw.is_empty() => {
                        let content: Value = serde_json::from_str(&raw).unwrap_or(json!(raw));
                        return Ok(extract_text_from_content(Some(&content)));
                    }
                    Ok(_) => {}
                    Err(err) => ctx.log(
                        "warn",
                        &format!(
                            "steering_checker: immutable version read unavailable for {file_version_id}, falling back to file head: {err}"
                        ),
                    ),
                }
            }

            if let Some(ref file_id) = msg_ref.content_file_id {
                if let Ok(raw) = read_content_file(ctx, temper_api_url, tenant, fields, file_id) {
                    if !raw.is_empty() {
                        let content: Value = serde_json::from_str(&raw).unwrap_or(json!(raw));
                        return Ok(extract_text_from_content(Some(&content)));
                    }
                }
            }
            if let Some(ref inline) = msg_ref.inline_content {
                return Ok(extract_text_from_content(Some(inline)));
            }
        }
        Ok(String::new())
    } else {
        let conversation_json = fields
            .get("conversation")
            .and_then(|v| v.as_str())
            .unwrap_or("[]");
        let messages: Vec<Value> = serde_json::from_str(conversation_json).unwrap_or_default();

        for msg in messages.iter().rev() {
            if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                return Ok(extract_text_from_content(msg.get("content")));
            }
        }
        Ok(String::new())
    }
}

/// Extract text from an assistant message content (handles both string and array formats).
fn extract_text_from_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    block.get("text").and_then(|v| v.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Simple token estimate (4 chars per token).
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

fn should_finalize_without_reply(session_id: &str, fields: &Value) -> bool {
    if string_field(fields, &["reply_channel_id", "ReplyChannelId"])
        .filter(|value| !value.trim().is_empty())
        .is_some()
        || string_field(fields, &["reply_thread_id", "ReplyThreadId"])
            .filter(|value| !value.trim().is_empty())
            .is_some()
        || string_field(fields, &["reply_channel_entity_id", "ReplyChannelEntityId"])
            .filter(|value| !value.trim().is_empty())
            .is_some()
        || string_field(fields, &["reply_channel_type", "ReplyChannelType"])
            .filter(|value| !value.trim().is_empty())
            .is_some()
    {
        return false;
    }

    let reply_route_source = string_field(fields, &["reply_route_source", "ReplyRouteSource"])
        .unwrap_or("")
        .trim();
    if !reply_route_source.is_empty() && reply_route_source != "direct_no_reply" {
        return false;
    }

    if string_field(fields, &["parent_session_id", "ParentSessionId"])
        .filter(|value| !value.trim().is_empty())
        .is_some()
    {
        return false;
    }

    let session_id = session_id.trim();
    if reply_route_source == "direct_no_reply" {
        return !session_id.is_empty();
    }

    let agent_id = string_field(fields, &["agent_id", "AgentId"])
        .unwrap_or("")
        .trim();
    !session_id.is_empty() && (agent_id.is_empty() || agent_id == session_id)
}

fn string_field<'a>(fields: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| fields.get(*name)?.as_str())
}
