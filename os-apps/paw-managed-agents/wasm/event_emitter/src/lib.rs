#[path = "../../common.rs"]
mod common;

use common::{
    content_string_to_text, create_session_event, extract_text_from_value, field_i64, field_string,
    get_entity, log_managed_session_event, managed_session_event_context, message_blocks_json,
    next_session_event_sequence, status_of, system_json_headers, with_session_event_context,
};
use session_tree_lib::{EntryType, SessionEntry, SessionTree};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{read_content_file, read_session_from_temperfs, resolve_temper_api_url};

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let base_url = resolve_temper_api_url(&ctx, &fields);
        let headers = system_json_headers(&ctx, &ctx.tenant, &fields);

        let inner_session_id = field_string(&fields, &["InnerSessionId", "inner_session_id"]);
        if inner_session_id.is_empty() {
            temper_wasm_sdk::set_success_result(
                "UpdateUsage",
                &json!({
                    "InputTokens": 0,
                    "OutputTokens": 0,
                    "StopReasonEventIds": "[]",
                    "LastEmittedResultHash": "missing-inner-session",
                }),
            );
            return Ok(());
        }

        let inner_session = get_entity(&ctx, &base_url, &headers, "Sessions", &inner_session_id)?;
        let inner_fields = inner_session
            .get("fields")
            .cloned()
            .unwrap_or_else(|| inner_session.clone());
        let inner_status = status_of(&inner_session);
        if inner_status != "Completed" {
            return Err(format!(
                "event_emitter only supports completed inner sessions, got status {inner_status}"
            ));
        }
        let tree_snapshot = load_session_tree_snapshot(&ctx, &base_url, &inner_fields)?;
        let result_text = resolve_result_text(&inner_fields, tree_snapshot.as_ref());
        let stop_reason = {
            let existing = field_string(&fields, &["StopReason", "stop_reason"]);
            if existing.is_empty() {
                "user_input_required".to_string()
            } else {
                existing
            }
        };
        let event_context = managed_session_event_context(
            &fields,
            &ctx.entity_id,
            &inner_session_id,
            "",
            "",
            "",
            "",
            "ManagedAgents.IdleSession",
        );
        let result_hash = format!("{inner_session_id}:{inner_status}:{result_text}");
        let previous_hash = field_string(
            &fields,
            &["LastEmittedResultHash", "last_emitted_result_hash"],
        );
        let previous_inner_session_id = field_string(
            &fields,
            &["LastEmittedInnerSessionId", "last_emitted_inner_session_id"],
        );
        let previous_tree_index = field_i64(
            &fields,
            &["LastEmittedTreeIndex", "last_emitted_tree_index"],
        )
        .max(0) as usize;
        let tree_index = tree_snapshot
            .as_ref()
            .map(|snapshot| snapshot.tree_index)
            .unwrap_or(0);
        let emit_from_index = emission_start_index(
            &inner_session_id,
            &previous_inner_session_id,
            previous_tree_index,
        );
        let has_new_tree_entries = (tree_index as usize) > emit_from_index;

        if previous_hash != result_hash || has_new_tree_entries {
            let mut sequence =
                next_session_event_sequence(&ctx, &base_url, &headers, &ctx.entity_id)?;
            if has_new_tree_entries {
                if let Some(snapshot) = tree_snapshot.as_ref() {
                    emit_tree_events(
                        &ctx,
                        &base_url,
                        &headers,
                        &inner_fields,
                        &ctx.entity_id,
                        &snapshot.tree,
                        emit_from_index,
                        &event_context,
                        &mut sequence,
                    )?;
                }
            }

            if previous_hash != result_hash && !result_text.trim().is_empty() {
                let event_fields = with_session_event_context(
                    &event_context,
                    json!({ "Content": message_blocks_json(&result_text) }),
                );
                let _ = create_session_event(
                    &ctx,
                    &base_url,
                    &headers,
                    &ctx.entity_id,
                    sequence,
                    "agent.message",
                    event_fields.clone(),
                )?;
                log_managed_session_event(
                    &ctx,
                    &event_context,
                    "agent.message",
                    sequence,
                    &event_fields,
                );
                sequence += 1;
            }

            if previous_hash != result_hash {
                let event_fields = with_session_event_context(
                    &event_context,
                    json!({
                        "StopReason": stop_reason,
                        "StopReasonEventIds": "[]",
                    }),
                );
                let _ = create_session_event(
                    &ctx,
                    &base_url,
                    &headers,
                    &ctx.entity_id,
                    sequence,
                    "session.status_idle",
                    event_fields.clone(),
                )?;
                log_managed_session_event(
                    &ctx,
                    &event_context,
                    "session.status_idle",
                    sequence,
                    &event_fields,
                );
            }
        }

        temper_wasm_sdk::set_success_result(
            "UpdateUsage",
            &json!({
                "InputTokens": field_i64(&inner_fields, &["InputTokens", "input_tokens"]),
                "OutputTokens": field_i64(&inner_fields, &["OutputTokens", "output_tokens"]),
                "StopReason": stop_reason,
                "StopReasonEventIds": "[]",
                "LastEmittedResultHash": result_hash,
                "LastEmittedInnerSessionId": inner_session_id,
                "LastEmittedTreeIndex": tree_index,
            }),
        );
        Ok(())
    })();

    if let Err(error) = result {
        temper_wasm_sdk::set_error_result(&error);
    }
    0
}

struct LoadedSessionTree {
    tree: SessionTree,
    result_text: String,
    tree_index: i64,
}

fn load_session_tree_snapshot(
    ctx: &Context,
    base_url: &str,
    inner_fields: &Value,
) -> Result<Option<LoadedSessionTree>, String> {
    let session_file_id = field_string(inner_fields, &["SessionFileId", "session_file_id"]);
    let session_leaf_id = field_string(inner_fields, &["SessionLeafId", "session_leaf_id"]);
    if session_file_id.is_empty() || session_leaf_id.is_empty() {
        return Ok(None);
    }

    let jsonl =
        read_session_from_temperfs(ctx, base_url, &ctx.tenant, inner_fields, &session_file_id)?;
    let tree = SessionTree::from_jsonl(&jsonl);
    let result_text =
        resolve_tree_result_text(ctx, base_url, inner_fields, &tree, &session_leaf_id)?;

    Ok(Some(LoadedSessionTree {
        tree_index: tree.entry_ids().len() as i64,
        tree,
        result_text,
    }))
}

fn resolve_result_text(inner_fields: &Value, snapshot: Option<&LoadedSessionTree>) -> String {
    let direct = field_string(inner_fields, &["Result", "result"]);
    if !direct.is_empty() {
        return direct;
    }

    snapshot
        .map(|tree| tree.result_text.clone())
        .unwrap_or_default()
}

fn resolve_tree_result_text(
    ctx: &Context,
    base_url: &str,
    inner_fields: &Value,
    tree: &SessionTree,
    session_leaf_id: &str,
) -> Result<String, String> {
    let refs = tree.build_context_refs(session_leaf_id);
    for item in refs.iter().rev() {
        if item.role != "assistant" {
            continue;
        }
        if let Some(file_id) = &item.content_file_id {
            let raw = read_content_file(ctx, base_url, &ctx.tenant, inner_fields, file_id)?;
            let text = content_string_to_text(&raw);
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
        if let Some(inline) = &item.inline_content {
            let text = extract_text_from_value(inline);
            if !text.trim().is_empty() {
                return Ok(text);
            }
        }
    }

    Ok(String::new())
}

fn emit_tree_events(
    ctx: &Context,
    base_url: &str,
    headers: &[(String, String)],
    inner_fields: &Value,
    session_id: &str,
    tree: &SessionTree,
    start_index: usize,
    event_context: &Value,
    sequence: &mut i64,
) -> Result<(), String> {
    for entry_id in tree.entry_ids().iter().skip(start_index) {
        let Some(entry) = tree.get(entry_id) else {
            continue;
        };
        if entry.entry_type != EntryType::Message {
            continue;
        }

        let role = entry.data.get("role").and_then(Value::as_str).unwrap_or("");
        let content = load_entry_content(ctx, base_url, inner_fields, entry)?;
        let Some(blocks) = content.as_array() else {
            continue;
        };

        match role {
            "assistant" => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str).unwrap_or("") {
                        "thinking" => {
                            let event_fields = with_session_event_context(
                                event_context,
                                json!({
                                    "Content": serde_json::to_string(&vec![block.clone()])
                                        .unwrap_or_else(|_| "[]".to_string()),
                                }),
                            );
                            let _ = create_session_event(
                                ctx,
                                base_url,
                                headers,
                                session_id,
                                *sequence,
                                "agent.thinking",
                                event_fields.clone(),
                            )?;
                            log_managed_session_event(
                                ctx,
                                event_context,
                                "agent.thinking",
                                *sequence,
                                &event_fields,
                            );
                            *sequence += 1;
                        }
                        "tool_use" => {
                            let event_fields = with_session_event_context(
                                event_context,
                                json!({
                                    "ToolUseId": block.get("id").and_then(Value::as_str).unwrap_or(""),
                                    "ToolName": block.get("name").and_then(Value::as_str).unwrap_or(""),
                                    "Input": block.get("input").map(|value| value.to_string()).unwrap_or_default(),
                                    "EvaluatedPermission": "allow",
                                }),
                            );
                            let _ = create_session_event(
                                ctx,
                                base_url,
                                headers,
                                session_id,
                                *sequence,
                                "agent.tool_use",
                                event_fields.clone(),
                            )?;
                            log_managed_session_event(
                                ctx,
                                event_context,
                                "agent.tool_use",
                                *sequence,
                                &event_fields,
                            );
                            *sequence += 1;
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let event_fields = with_session_event_context(
                        event_context,
                        json!({
                            "ToolUseId": block.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
                            "Content": message_blocks_json(
                                &block
                                    .get("content")
                                    .map(extract_text_from_value)
                                    .unwrap_or_default()
                            ),
                        }),
                    );
                    let _ = create_session_event(
                        ctx,
                        base_url,
                        headers,
                        session_id,
                        *sequence,
                        "agent.tool_result",
                        event_fields.clone(),
                    )?;
                    log_managed_session_event(
                        ctx,
                        event_context,
                        "agent.tool_result",
                        *sequence,
                        &event_fields,
                    );
                    *sequence += 1;
                }
            }
            _ => {}
        }
    }

    Ok(())
}
fn emission_start_index(
    current_inner_session_id: &str,
    previous_inner_session_id: &str,
    previous_tree_index: usize,
) -> usize {
    if current_inner_session_id == previous_inner_session_id {
        previous_tree_index
    } else {
        0
    }
}

fn load_entry_content(
    ctx: &Context,
    base_url: &str,
    inner_fields: &Value,
    entry: &SessionEntry,
) -> Result<Value, String> {
    if let Some(file_id) = &entry.content_file_id {
        let raw = read_content_file(ctx, base_url, &ctx.tenant, inner_fields, file_id)?;
        return serde_json::from_str(&raw).or_else(|_| Ok(json!(raw)));
    }

    Ok(entry
        .data
        .get("content")
        .cloned()
        .unwrap_or_else(|| json!(null)))
}
