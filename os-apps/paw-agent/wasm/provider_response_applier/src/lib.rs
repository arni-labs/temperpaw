//! Provider Response Applier — staged Session-turn WASM for persistence and routing.
//!
//! Owns the `ApplyingProviderResponse` phase:
//! - read prepared/provider-response artifacts
//! - append assistant output back into session storage
//! - externalize oversized assistant content when needed
//! - derive the next Session action
//! - route to `ProcessToolCalls`, `CheckSteering`, `RecordResult`, `RecordResultNoReply`,
//!   or `RecordResultInlineReply`
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use session_tree_lib::SessionTree;
use session_turn_artifacts::{
    PreparedContextArtifact, ProviderResponseArtifact, build_provider_response_applier_base_params,
    parse_prepared_context_artifact, parse_provider_response_artifact,
};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    MAX_ENTRY_EXTRA_BYTES, append_session_entry_inline, create_content_file,
    is_session_entries_ref, materialize_initial_session_entries_with_assistant, read_content_file,
    read_session_from_temperfs, resolve_temper_api_url, runtime_headers,
    session_id_from_entries_ref, stored_json_len as escaped_json_len, write_session_to_temperfs,
    write_temperfs_value_with_retry,
};

const SESSION_ENTRY_FILE_THRESHOLD_BYTES: usize = 4096;
const DEFAULT_PROVIDER_RESPONSE_APPLY_BUDGET_MS: i64 = 30_000;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_provider_response_applier() {
        set_error_result(&err);
    }
    0
}

pub fn run_provider_response_applier() -> Result<(), String> {
    let started_at = Context::get_time_millis();
    let ctx = Context::from_host()?;
    ctx.log("info", "provider_response_applier: starting");

    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let prepared_context_file_id = fields
        .get("prepared_context_file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let provider_response_file_id = fields
        .get("provider_response_file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let prepared_context_inline_json =
        read_state_string_field(&ctx, &fields, "prepared_context_inline_json");
    let provider_response_inline_json =
        read_state_string_field(&ctx, &fields, "provider_response_inline_json");
    if (prepared_context_file_id.is_empty() && prepared_context_inline_json.is_empty())
        || (provider_response_file_id.is_empty() && provider_response_inline_json.is_empty())
    {
        return Err(
            "provider_response_applier: missing prepared/provider response inline JSON or file IDs"
                .to_string(),
        );
    }

    let temper_api_url = resolve_temper_api_url(&ctx, &fields);
    let tenant = &ctx.tenant;
    let apply_budget_ms = configured_budget_ms(
        &ctx,
        &fields,
        "provider_response_apply_budget_ms",
        DEFAULT_PROVIDER_RESPONSE_APPLY_BUDGET_MS,
    );
    let read_prepared_started_at = Context::get_time_millis();
    let prepared_result = read_prepared_context_artifact(
        &ctx,
        &temper_api_url,
        tenant,
        &fields,
        prepared_context_file_id,
        &prepared_context_inline_json,
    );
    emit_phase_step_duration(
        &ctx,
        "provider_response_applier",
        "read_prepared_artifact",
        read_prepared_started_at,
        if prepared_result.is_ok() {
            "ok"
        } else {
            "error"
        },
    );
    let prepared = prepared_result?;
    check_phase_budget(
        &ctx,
        "provider_response_applier",
        started_at,
        apply_budget_ms,
        "read_prepared_artifact",
    )?;

    let read_response_started_at = Context::get_time_millis();
    let response_result = read_provider_response_artifact(
        &ctx,
        &temper_api_url,
        tenant,
        &fields,
        provider_response_file_id,
        &provider_response_inline_json,
    );
    emit_phase_step_duration(
        &ctx,
        "provider_response_applier",
        "read_provider_response_artifact",
        read_response_started_at,
        if response_result.is_ok() {
            "ok"
        } else {
            "error"
        },
    );
    let response = response_result?;
    check_phase_budget(
        &ctx,
        "provider_response_applier",
        started_at,
        apply_budget_ms,
        "read_provider_response_artifact",
    )?;

    let legacy_conversation = legacy_updated_conversation_payload(&prepared, &response);
    if let Some(ref updated_conversation) = legacy_conversation
        && !prepared.conversation_file_id.is_empty()
    {
        write_conversation_to_temperfs(
            &ctx,
            &temper_api_url,
            tenant,
            &fields,
            &prepared.conversation_file_id,
            updated_conversation,
        )?;
    }
    let inline_conversation =
        if !prepared.use_session_tree && prepared.conversation_file_id.is_empty() {
            legacy_conversation
        } else {
            None
        };

    match response.stop_reason.as_str() {
        "tool_use" => {
            let tool_calls = extract_tool_calls(&response.content);
            let append_started_at = Context::get_time_millis();
            let append_result = append_assistant_response_to_session_tree(
                &ctx,
                &prepared,
                &temper_api_url,
                tenant,
                &fields,
                &response,
            );
            emit_phase_step_duration(
                &ctx,
                "provider_response_applier",
                "append_session_tree",
                append_started_at,
                if append_result.is_ok() { "ok" } else { "error" },
            );
            let new_leaf = append_result?;
            note_phase_budget_overrun_after_committed_step(
                &ctx,
                "provider_response_applier",
                started_at,
                apply_budget_ms,
                "append_session_tree",
            );

            let mut params = build_provider_response_applier_base_params(&prepared, &response);
            carry_reply_attachments(&mut params, &fields);
            params["pending_tool_calls"] =
                json!(serde_json::to_string(&tool_calls).unwrap_or_default());
            if let Some(leaf) = new_leaf {
                params["session_leaf_id"] = json!(leaf);
                params["session_entries_materialized"] = json!("true");
            }
            if let Some(conversation) = inline_conversation {
                params["conversation"] = json!(conversation);
            }
            set_success_result("ProcessToolCalls", &params);
            emit_phase_total_duration(
                &ctx,
                "provider_response_applier",
                started_at,
                "process_tool_calls",
            );
        }
        "end_turn" | "stop" => {
            let result_text = extract_text_response(&response.content);
            let append_started_at = Context::get_time_millis();
            let append_result = append_assistant_response_to_session_tree(
                &ctx,
                &prepared,
                &temper_api_url,
                tenant,
                &fields,
                &response,
            );
            emit_phase_step_duration(
                &ctx,
                "provider_response_applier",
                "append_session_tree",
                append_started_at,
                if append_result.is_ok() { "ok" } else { "error" },
            );
            let new_leaf = append_result?;
            note_phase_budget_overrun_after_committed_step(
                &ctx,
                "provider_response_applier",
                started_at,
                apply_budget_ms,
                "append_session_tree",
            );

            let mut params = build_provider_response_applier_base_params(&prepared, &response);
            carry_reply_attachments(&mut params, &fields);
            params["result"] = json!(result_text);
            match new_leaf {
                Some(leaf) => {
                    params["session_leaf_id"] = json!(leaf);
                    params["session_entries_materialized"] = json!("true");
                }
                None => {
                    params["session_leaf_id"] = Value::Null;
                }
            }

            if should_check_steering(&fields) {
                set_success_result("CheckSteering", &params);
                emit_phase_total_duration(
                    &ctx,
                    "provider_response_applier",
                    started_at,
                    "check_steering",
                );
            } else {
                if let Some(conversation) = inline_conversation {
                    params["conversation"] = json!(conversation);
                }
                let terminal_action = if should_bypass_terminal_reply(&ctx.entity_id, &fields) {
                    "RecordResultNoReply"
                } else if try_dispatch_inline_reply(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    &fields,
                    &result_text,
                ) {
                    "RecordResultInlineReply"
                } else {
                    "RecordResult"
                };
                set_success_result(terminal_action, &params);
                emit_phase_total_duration(
                    &ctx,
                    "provider_response_applier",
                    started_at,
                    terminal_phase_name(terminal_action),
                );
            }
        }
        other => return Err(format!("unsupported stop_reason: {other}")),
    }

    Ok(())
}

fn read_prepared_context_artifact(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
    inline_json: &str,
) -> Result<PreparedContextArtifact, String> {
    let raw = if inline_json.is_empty() {
        read_content_file(ctx, temper_api_url, tenant, fields, file_id)?
    } else {
        inline_json.to_string()
    };
    parse_prepared_context_artifact(&raw)
}

fn read_provider_response_artifact(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
    inline_json: &str,
) -> Result<ProviderResponseArtifact, String> {
    let raw = if inline_json.is_empty() {
        read_content_file(ctx, temper_api_url, tenant, fields, file_id)?
    } else {
        inline_json.to_string()
    };
    parse_provider_response_artifact(&raw)
}

fn read_state_string_field(ctx: &Context, fields: &Value, field_name: &str) -> String {
    match ctx.read_field_string(field_name) {
        Ok(value) if !value.is_empty() => value,
        _ => fields
            .get(field_name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn session_entries_materialized(fields: &Value) -> bool {
    fields
        .get("session_entries_materialized")
        .and_then(Value::as_str)
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

fn initial_user_message(fields: &Value, prepared: &PreparedContextArtifact) -> Result<String, String> {
    if let Some(message) = fields
        .get("user_message")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Ok(message.to_string());
    }

    prepared
        .messages
        .iter()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .map(|content| match content.as_str() {
            Some(text) => text.to_string(),
            None => serde_json::to_string(content).unwrap_or_default(),
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "virtual first-turn SessionEntries materialization requires a user message"
                .to_string()
        })
}

fn legacy_updated_conversation_payload(
    prepared: &PreparedContextArtifact,
    artifact: &ProviderResponseArtifact,
) -> Option<String> {
    if prepared.use_session_tree {
        return None;
    }

    let mut messages = prepared.messages.clone();
    messages.push(json!({
        "role": "assistant",
        "content": artifact.content.clone(),
    }));
    Some(serde_json::to_string(&messages).unwrap_or_default())
}

fn append_assistant_response_to_session_tree(
    ctx: &Context,
    prepared: &PreparedContextArtifact,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    response: &ProviderResponseArtifact,
) -> Result<Option<String>, String> {
    if !prepared.use_session_tree {
        return Ok(None);
    }

    let content = &response.content;
    let output_tokens = response.output_tokens.max(0) as usize;
    let extra = assistant_turn_extra(response, Context::get_time_millis());
    let extra = Some(&extra);

    if is_session_entries_ref(&prepared.session_file_id) {
        if !session_entries_materialized(fields) {
            let session_id = session_id_from_entries_ref(&prepared.session_file_id)
                .ok_or("session-entries reference missing session id")?;
            let user_message = initial_user_message(fields, prepared)?;
            let created = materialize_initial_session_entries_with_assistant(
                ctx,
                temper_api_url,
                tenant,
                fields,
                session_id,
                &user_message,
                content,
                output_tokens,
                extra,
            )?;
            ctx.log(
                "info",
                &format!(
                    "provider_response_applier: materialized virtual first-turn SessionEntries through assistant leaf {}",
                    created.entry_id
                ),
            );
            return Ok(Some(created.entry_id));
        }
        let created = append_session_entry_inline(
            ctx,
            temper_api_url,
            tenant,
            fields,
            &prepared.session_file_id,
            &prepared.session_leaf_id,
            "a",
            "assistant",
            content,
            output_tokens,
            extra,
        )?;
        return Ok(Some(created.entry_id));
    }

    let session_jsonl = read_session_from_temperfs(
        ctx,
        temper_api_url,
        tenant,
        fields,
        &prepared.session_file_id,
    )?;
    let mut tree = SessionTree::from_jsonl(&session_jsonl);
    let content_str = serde_json::to_string(content).unwrap_or_default();
    let entity_backed_session = is_session_entries_ref(&prepared.session_file_id);
    let (new_leaf, externalized) = if !entity_backed_session
        && !prepared.workspace_id.is_empty()
        && should_store_entry_as_file(&content_str)
    {
        match create_content_file_for_entry(
            ctx,
            temper_api_url,
            tenant,
            &prepared.workspace_id,
            &format!("a-{}", tree.len()),
            &content_str,
        ) {
            Ok(content_file_id) => {
                let (leaf, _) = tree.append_assistant_message_file(
                    &prepared.session_leaf_id,
                    &content_file_id,
                    None,
                    output_tokens,
                    extra,
                );
                (leaf, true)
            }
            Err(_) => {
                let (leaf, _) = tree.append_assistant_message_with_extra(
                    &prepared.session_leaf_id,
                    content,
                    output_tokens,
                    extra,
                );
                (leaf, false)
            }
        }
    } else {
        let (leaf, _) = tree.append_assistant_message_with_extra(
            &prepared.session_leaf_id,
            content,
            output_tokens,
            extra,
        );
        (leaf, false)
    };

    if externalized {
        emit_metric_ignore(
            ctx,
            "temper_session_large_content_externalized_total",
            1.0,
            &session_metric_tags("", ""),
            Some("count"),
        );
    }

    write_session_to_temperfs(
        ctx,
        temper_api_url,
        tenant,
        fields,
        &prepared.session_file_id,
        &tree.to_jsonl(),
    )?;
    Ok(Some(new_leaf))
}

/// Ceiling on a single token-level signal array stored on a SessionEntry.
///
/// These arrays scale with completion length, and the entry's ExtraJson has its
/// own overflow ceiling; a long completion's logprobs must not be what pushes a
/// turn over it.
const MAX_TOKEN_SIGNAL_BYTES: usize = 32_768;

// The entry's `extra_json` ceiling (`MAX_ENTRY_EXTRA_BYTES`) is spent here as
// policy: bounding each signal on its own does not bound their sum — four
// signals just under the per-signal ceiling each pass and cross the entry
// ceiling together — and past it the kernel replaces or externalizes the
// *entire* field, taking the per-turn facts the OTS emitter needs with it.
// Choosing which signal to sacrifice, and naming it, belongs to this writer;
// `wasm_helpers` enforces the same ceiling at the write boundary for every
// writer, including ones that never come through here.

/// Headroom withheld from `MAX_ENTRY_EXTRA_BYTES`.
///
/// `create_session_entry` stamps `recorded_at` onto the object after this
/// function has returned, and the `<signal>_dropped_bytes` markers written
/// below are themselves not charged against the budget. Escaping is not part of
/// the headroom — `escaped_json_len` accounts for it exactly.
const ENTRY_EXTRA_HEADROOM_BYTES: usize = 4_096;

/// Per-turn facts recorded on the assistant SessionEntry.
///
/// The OTS emitter reads these back to date each turn, report its prompt and
/// completion token counts, and carry token-level RL signals when the serving
/// stack produced them. Everything here is already in hand — recording it costs
/// no extra provider or storage round trip. `now_ms` is passed in rather than
/// read here so the mapping stays testable off-host.
fn assistant_turn_extra(response: &ProviderResponseArtifact, now_ms: i64) -> Value {
    let mut extra = json!({
        "ts_ms": now_ms,
        "provider": response.provider,
        "model": response.model,
        "stop_reason": response.stop_reason,
        "input_tokens": response.input_tokens.max(0),
        "output_tokens": response.output_tokens.max(0),
    });
    if response.cache_read_input_tokens > 0 {
        extra["cache_read_input_tokens"] = json!(response.cache_read_input_tokens);
    }
    if response.cache_creation_input_tokens > 0 {
        extra["cache_creation_input_tokens"] = json!(response.cache_creation_input_tokens);
    }
    if let Some(Value::Object(signals)) = response.token_signals.clone() {
        // Signals are added against a running total, so the entry keeps as many
        // as fit and the ones that do not fit are named. The per-turn facts
        // above are never at risk: they are already in the object, and nothing
        // below can push the value past the ceiling.
        let mut remaining = MAX_ENTRY_EXTRA_BYTES
            .saturating_sub(ENTRY_EXTRA_HEADROOM_BYTES)
            .saturating_sub(escaped_json_len(&extra));
        for (key, value) in signals {
            let size = serde_json::to_string(&value).map(|json| json.len()).unwrap_or(0);
            // The key, quotes, colon and separator ride along with the value,
            // and the kernel measures the field after JSON-escaping it.
            let cost = escaped_json_len(&value) + key.len() + 4;
            let dropped = if size > MAX_TOKEN_SIGNAL_BYTES || cost > remaining {
                true
            } else {
                remaining -= cost;
                false
            };
            let Some(target) = extra.as_object_mut() else {
                break;
            };
            if dropped {
                // Record that it existed and how big it was; a dropped signal
                // that leaves a trace is debuggable, a silent one is not.
                target.insert(format!("{key}_dropped_bytes"), json!(size));
                continue;
            }
            target.insert(key, value);
        }
    }
    extra
}


fn extract_tool_calls(content: &Value) -> Vec<Value> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
        .cloned()
        .collect()
}

fn extract_text_response(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| {
            (block.get("type").and_then(|v| v.as_str()) == Some("text"))
                .then(|| block.get("text").and_then(|v| v.as_str()))
                .flatten()
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn should_check_steering(fields: &Value) -> bool {
    let max_follow_ups = field_i64(fields, "max_follow_ups").unwrap_or(5);
    if max_follow_ups <= 0 {
        return false;
    }

    let follow_up_count = field_i64(fields, "follow_up_count").unwrap_or(0);
    follow_up_count < max_follow_ups && has_queued_steering_messages(fields)
}

fn has_queued_steering_messages(fields: &Value) -> bool {
    let Some(raw) = fields.get("steering_messages").and_then(Value::as_str) else {
        return false;
    };
    serde_json::from_str::<Vec<Value>>(raw)
        .map(|messages| !messages.is_empty())
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineReplyRoute {
    channel_id: String,
    channel_entity_id: String,
    channel_type: String,
    thread_id: String,
    agent_entity_id: String,
}

fn try_dispatch_inline_reply(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    reply_text: &str,
) -> bool {
    let Some(route) = inline_reply_route(&ctx.entity_id, fields) else {
        return false;
    };

    let url = inline_reply_action_url(temper_api_url, &route.channel_entity_id);
    let mut headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    if !route.channel_id.is_empty() {
        headers.push((
            "x-temper-attr-channelid".to_string(),
            route.channel_id.clone(),
        ));
    }

    let body = inline_reply_body(&route, reply_text);
    match ctx.http_call("POST", &url, &headers, &body.to_string()) {
        Ok(resp) if (200..300).contains(&resp.status) => {
            ctx.log(
                "info",
                &format!(
                    "provider_response_applier: recorded inline reply for thread {} via {}",
                    route.thread_id, route.channel_type
                ),
            );
            true
        }
        Ok(resp) => {
            ctx.log(
                "warn",
                &format!(
                    "provider_response_applier: inline reply dispatch failed; falling back to RecordResult (HTTP {}): {}",
                    resp.status,
                    truncate_body(&resp.body)
                ),
            );
            false
        }
        Err(err) => {
            ctx.log(
                "warn",
                &format!(
                    "provider_response_applier: inline reply dispatch failed; falling back to RecordResult: {err}"
                ),
            );
            false
        }
    }
}

fn inline_reply_route(session_id: &str, fields: &Value) -> Option<InlineReplyRoute> {
    let channel_type = trimmed_string_field(fields, &["reply_channel_type", "ReplyChannelType"])?;
    if !matches!(channel_type.trim(), "cli" | "tui") {
        return None;
    }

    let channel_entity_id =
        trimmed_string_field(fields, &["reply_channel_entity_id", "ReplyChannelEntityId"])?;
    let thread_id = trimmed_string_field(fields, &["reply_thread_id", "ReplyThreadId"])?;
    let channel_id =
        trimmed_string_field(fields, &["reply_channel_id", "ReplyChannelId"]).unwrap_or_default();
    let agent_entity_id = string_field(fields, &["agent_id", "AgentId"])
        .unwrap_or(session_id)
        .trim()
        .to_string();

    Some(InlineReplyRoute {
        channel_id,
        channel_entity_id,
        channel_type,
        thread_id,
        agent_entity_id,
    })
}

fn trimmed_string_field(fields: &Value, names: &[&str]) -> Option<String> {
    string_field(fields, names)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn inline_reply_action_url(temper_api_url: &str, channel_entity_id: &str) -> String {
    format!(
        "{temper_api_url}/tdata/Channels('{}')/Paw.Channel.ReplyDelivered",
        escape_odata(channel_entity_id)
    )
}

fn inline_reply_body(route: &InlineReplyRoute, reply_text: &str) -> Value {
    json!({
        "thread_id": route.thread_id.as_str(),
        "content": reply_text,
        "agent_entity_id": route.agent_entity_id.as_str(),
    })
}

fn terminal_phase_name(action: &str) -> &'static str {
    match action {
        "RecordResultNoReply" => "record_result_no_reply",
        "RecordResultInlineReply" => "record_result_inline_reply",
        _ => "record_result",
    }
}

fn should_bypass_terminal_reply(session_id: &str, fields: &Value) -> bool {
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

fn carry_reply_attachments(params: &mut Value, fields: &Value) {
    if let Some(value) = string_field(fields, &["reply_attachments_json", "ReplyAttachmentsJson"])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        params["reply_attachments_json"] = json!(value);
    }
}

fn escape_odata(value: &str) -> String {
    value.replace('\'', "''")
}

fn truncate_body(body: &str) -> String {
    const LIMIT: usize = 240;
    if body.len() <= LIMIT {
        body.to_string()
    } else {
        format!("{}...", &body[..LIMIT])
    }
}

fn field_i64(fields: &Value, field_name: &str) -> Option<i64> {
    fields
        .get(field_name)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
}

fn write_conversation_to_temperfs(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    file_id: &str,
    conversation_json: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = runtime_headers(ctx, tenant, fields, Some("application/json"), None);
    let body = format!("{{\"messages\":{conversation_json}}}");
    write_temperfs_value_with_retry(ctx, &url, &headers, &body, "TemperFS conversation write")?;
    Ok(())
}

fn create_content_file_for_entry(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    entry_id: &str,
    content: &str,
) -> Result<String, String> {
    create_content_file(
        ctx,
        temper_api_url,
        tenant,
        workspace_id,
        &format!("msg-{entry_id}.txt"),
        content,
    )
}

fn should_store_entry_as_file(content: &str) -> bool {
    content.len() > SESSION_ENTRY_FILE_THRESHOLD_BYTES
}

fn session_metric_tags(provider: &str, model: &str) -> Value {
    json!({
        "provider": provider,
        "model": model,
    })
}

fn emit_metric_ignore(ctx: &Context, name: &str, value: f64, tags: &Value, kind: Option<&str>) {
    let _ = ctx.emit_metric(name, value, tags, kind);
}

fn elapsed_ms_since(started_at: i64) -> i64 {
    Context::get_time_millis().saturating_sub(started_at)
}

fn configured_budget_ms(ctx: &Context, fields: &Value, key: &str, default_value: i64) -> i64 {
    fields
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| ctx.config.get(key).and_then(|s| s.parse::<i64>().ok()))
        .filter(|value| *value > 0)
        .unwrap_or(default_value)
}

fn emit_phase_step_duration(
    ctx: &Context,
    phase: &str,
    step: &str,
    started_at: i64,
    result: &str,
) -> i64 {
    let elapsed_ms = elapsed_ms_since(started_at);
    emit_metric_ignore(
        ctx,
        "temper_session_phase_step_duration_ms",
        elapsed_ms as f64,
        &json!({
            "phase": phase,
            "step": step,
            "result": result,
        }),
        Some("histogram"),
    );
    ctx.log(
        "info",
        &format!("session_phase phase={phase} step={step} result={result} elapsed_ms={elapsed_ms}"),
    );
    elapsed_ms
}

fn emit_phase_total_duration(ctx: &Context, phase: &str, started_at: i64, result: &str) -> i64 {
    let elapsed_ms = elapsed_ms_since(started_at);
    emit_metric_ignore(
        ctx,
        "temper_session_phase_duration_ms",
        elapsed_ms as f64,
        &json!({
            "phase": phase,
            "result": result,
        }),
        Some("histogram"),
    );
    elapsed_ms
}

fn check_phase_budget(
    ctx: &Context,
    phase: &str,
    started_at: i64,
    budget_ms: i64,
    last_step: &str,
) -> Result<(), String> {
    let elapsed_ms = elapsed_ms_since(started_at);
    if elapsed_ms <= budget_ms {
        return Ok(());
    }

    emit_metric_ignore(
        ctx,
        "temper_session_phase_budget_exceeded_total",
        1.0,
        &json!({
            "phase": phase,
            "last_step": last_step,
        }),
        Some("count"),
    );
    Err(format!(
        "{phase}: exceeded local budget after {last_step} (elapsed_ms={elapsed_ms}, budget_ms={budget_ms})"
    ))
}

fn note_phase_budget_overrun_after_committed_step(
    ctx: &Context,
    phase: &str,
    started_at: i64,
    budget_ms: i64,
    committed_step: &str,
) {
    let elapsed_ms = elapsed_ms_since(started_at);
    if elapsed_ms <= budget_ms {
        return;
    }

    emit_metric_ignore(
        ctx,
        "temper_session_phase_budget_exceeded_after_commit_total",
        1.0,
        &json!({
            "phase": phase,
            "committed_step": committed_step,
        }),
        Some("count"),
    );
    ctx.log(
        "warn",
        &format!(
            "{phase}: exceeded local budget after committed {committed_step}; continuing because the session append already succeeded (elapsed_ms={elapsed_ms}, budget_ms={budget_ms})"
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_with_signals(token_signals: Option<Value>) -> ProviderResponseArtifact {
        ProviderResponseArtifact {
            version: 1,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            content: json!([{"type": "text", "text": "done"}]),
            stop_reason: "end_turn".to_string(),
            input_tokens: 120,
            output_tokens: 34,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 256,
            response_bytes: 512,
            token_signals,
        }
    }

    #[test]
    fn assistant_turn_extra_records_the_facts_the_emitter_needs() {
        let extra = assistant_turn_extra(&artifact_with_signals(None), 1_767_225_600_000);
        assert_eq!(extra["provider"], "anthropic");
        assert_eq!(extra["model"], "claude-sonnet-4-6");
        assert_eq!(extra["stop_reason"], "end_turn");
        assert_eq!(extra["input_tokens"], 120);
        assert_eq!(extra["output_tokens"], 34);
        assert_eq!(extra["ts_ms"], 1_767_225_600_000_i64, "turns must be datable");
        assert!(extra.get("logprobs").is_none());
    }

    #[test]
    fn assistant_turn_extra_carries_token_signals_when_present() {
        let extra = assistant_turn_extra(
            &artifact_with_signals(Some(json!({
                "logprobs": [-0.5, -1.25],
                "completion_token_ids": [7, 8],
            }))),
            1_767_225_600_000,
        );
        assert_eq!(extra["logprobs"], json!([-0.5, -1.25]));
        assert_eq!(extra["completion_token_ids"], json!([7, 8]));
    }

    /// The per-signal ceiling is applied to each signal on its own, and the ids
    /// are several times the size of the mask — so there is a completion length
    /// at which the ids are refused and the mask survives. That leaves an
    /// anchorless set, which the kernel rejects outright: the emitter has to
    /// drop it rather than pass it on, and this test pins that the writer really
    /// does produce that shape, so the emitter's guard is not guarding a
    /// hypothetical.
    #[test]
    fn assistant_turn_extra_can_keep_a_mask_after_refusing_its_token_ids() {
        // Roughly an 8,000-token completion: five-digit ids plus separators run
        // past the ceiling, a binary mask does not.
        let ids: Vec<Value> = (0..8_000).map(|i| json!(10_000 + (i % 50_000))).collect();
        let mask: Vec<Value> = (0..8_000).map(|_| json!(1)).collect();
        let ids_bytes = serde_json::to_string(&ids).unwrap().len();
        let mask_bytes = serde_json::to_string(&mask).unwrap().len();
        assert!(
            ids_bytes > MAX_TOKEN_SIGNAL_BYTES && mask_bytes <= MAX_TOKEN_SIGNAL_BYTES,
            "fixture must straddle the ceiling: ids={ids_bytes} mask={mask_bytes}"
        );

        let extra = assistant_turn_extra(
            &artifact_with_signals(Some(json!({
                "completion_token_ids": ids,
                "response_mask": mask,
            }))),
            1_767_225_600_000,
        );

        assert!(
            extra.get("completion_token_ids").is_none(),
            "the ids are over the per-signal ceiling and are refused"
        );
        assert!(
            extra["completion_token_ids_dropped_bytes"].as_u64().unwrap() > 0,
            "and the refusal is recorded so the emitter can see it"
        );
        assert!(
            extra["response_mask"].is_array(),
            "the mask fits, so the writer keeps it — this is the anchorless set \
             the emitter must not pass to the kernel"
        );
    }

    #[test]
    fn assistant_turn_extra_drops_oversized_token_signals() {
        let huge: Vec<Value> = (0..MAX_TOKEN_SIGNAL_BYTES).map(|i| json!(i % 10)).collect();
        let extra = assistant_turn_extra(
            &artifact_with_signals(Some(json!({
                "logprobs": huge,
                "completion_token_ids": [1, 2, 3],
            }))),
            1_767_225_600_000,
        );
        assert!(
            extra.get("logprobs").is_none(),
            "an oversized signal must not be written to the entity"
        );
        assert!(
            extra["logprobs_dropped_bytes"].as_u64().unwrap() > MAX_TOKEN_SIGNAL_BYTES as u64,
            "the drop must leave a trace"
        );
        assert_eq!(
            extra["completion_token_ids"],
            json!([1, 2, 3]),
            "a signal that fits still gets recorded"
        );
    }

    /// Four signals that each clear the per-signal ceiling still cross the
    /// entry's own ceiling together. Past it the kernel replaces or
    /// externalizes the whole `extra_json` value, so the per-turn facts go with
    /// them — the turn loses its timestamp, provider, model and token counts
    /// because of signals nothing was even asking for.
    #[test]
    fn assistant_turn_extra_bounds_signals_against_the_entry_ceiling() {
        // Single-digit elements serialize to two bytes each, so this lands just
        // under the per-signal ceiling: every one of these passes the individual
        // check, and four of them do not fit the entry together.
        let near_ceiling: Vec<Value> = (0..MAX_TOKEN_SIGNAL_BYTES / 2 - 8)
            .map(|i| json!(i % 10))
            .collect();
        assert!(
            serde_json::to_string(&near_ceiling).unwrap().len() <= MAX_TOKEN_SIGNAL_BYTES,
            "the fixture has to clear the per-signal ceiling for the test to mean anything"
        );
        let extra = assistant_turn_extra(
            &artifact_with_signals(Some(json!({
                "prompt_token_ids": near_ceiling,
                "completion_token_ids": near_ceiling,
                "response_mask": near_ceiling,
                "logprobs": near_ceiling,
            }))),
            1_767_225_600_000,
        );

        // Measured the way the kernel measures it: the field is a JSON string,
        // so the ceiling applies to the escaped encoding.
        let size = escaped_json_len(&extra);
        assert!(
            size <= MAX_ENTRY_EXTRA_BYTES - ENTRY_EXTRA_HEADROOM_BYTES,
            "extra_json must stay under the entry ceiling, got {size} bytes"
        );
        assert_eq!(
            extra["ts_ms"], 1_767_225_600_000_i64,
            "the per-turn facts must survive whatever the signals do"
        );
        assert_eq!(extra["provider"], "anthropic");
        assert_eq!(extra["input_tokens"], 120);

        let dropped: Vec<&String> = extra
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| key.ends_with("_dropped_bytes"))
            .collect();
        assert!(
            !dropped.is_empty(),
            "signals that did not fit must name themselves: {extra}"
        );
        for key in dropped {
            let signal = key.trim_end_matches("_dropped_bytes");
            assert!(
                extra.get(signal).is_none(),
                "{signal} must not be both written and reported dropped"
            );
        }
    }

    /// The kernel measures `extra_json` after encoding it as a JSON string, so
    /// a quote-dense value costs more stored bytes than it serializes to. A
    /// budget that counts the unescaped length would let such a value cross the
    /// ceiling and take the whole field — per-turn facts included — with it.
    #[test]
    fn entry_extra_budget_counts_escaped_bytes() {
        // Ground truth: what the kernel stores is the extras JSON encoded again
        // as a JSON string, which is what its overflow ceiling measures.
        for value in [json!("\"\"\"\"\"\"\"\""), json!("\n"), json!({"a": [1, 2]})] {
            let inner = serde_json::to_string(&value).unwrap();
            let stored = serde_json::to_string(&Value::String(inner)).unwrap();
            assert_eq!(
                escaped_json_len(&value),
                stored.len(),
                "escaped size must match the encoding the kernel measures for {value}"
            );
        }

        // A signal of quote-heavy strings: rejected at capture, and bounded
        // here as a second line of defence.
        let dense: Vec<Value> = (0..MAX_TOKEN_SIGNAL_BYTES / 8)
            .map(|_| json!("\"\"\""))
            .collect();
        let extra = assistant_turn_extra(
            &artifact_with_signals(Some(json!({
                "prompt_token_ids": dense.clone(),
                "completion_token_ids": dense.clone(),
                "response_mask": dense.clone(),
                "logprobs": dense,
            }))),
            1_767_225_600_000,
        );
        let size = escaped_json_len(&extra);
        assert!(
            size <= MAX_ENTRY_EXTRA_BYTES - ENTRY_EXTRA_HEADROOM_BYTES,
            "escaped extra_json must stay under the entry ceiling, got {size} bytes"
        );
        assert_eq!(extra["ts_ms"], 1_767_225_600_000_i64);
    }

    /// The ceiling is the spec's, not a number of this module's own choosing.
    /// (It equals the kernel's default field ceiling too, so the bound holds
    /// whichever of the two applies to this write path.)
    #[test]
    fn entry_extra_ceiling_matches_the_session_entry_spec() {
        let spec = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../specs/session_entry.ioa.toml"
        ))
        .expect("session_entry.ioa.toml should exist");
        let extra_json_block = spec
            .split("[[state]]")
            .find(|block| block.contains("name = \"extra_json\""))
            .expect("session_entry.ioa.toml should declare extra_json");
        assert!(
            extra_json_block
                .contains(&format!("overflow_inline_max_bytes = \"{MAX_ENTRY_EXTRA_BYTES}\"")),
            "MAX_ENTRY_EXTRA_BYTES must track the extra_json overflow ceiling: {extra_json_block}"
        );
    }

    #[test]
    fn extracts_tool_calls_only() {
        let tool_calls = extract_tool_calls(&json!([
            {"type": "text", "text": "hello"},
            {"type": "tool_use", "id": "tool-1", "name": "temper.get", "input": {"id": "x"}},
            {"type": "tool_use", "id": "tool-2", "name": "temper.list", "input": {}}
        ]));

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0]["id"], "tool-1");
        assert_eq!(tool_calls[1]["id"], "tool-2");
    }

    #[test]
    fn extracts_text_response_blocks() {
        let text = extract_text_response(&json!([
            {"type": "text", "text": "first"},
            {"type": "tool_use", "id": "tool-1"},
            {"type": "text", "text": "second"}
        ]));

        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn stores_large_entries_as_files_only_after_threshold() {
        assert!(!should_store_entry_as_file(
            &"a".repeat(SESSION_ENTRY_FILE_THRESHOLD_BYTES)
        ));
        assert!(should_store_entry_as_file(
            &"a".repeat(SESSION_ENTRY_FILE_THRESHOLD_BYTES + 1)
        ));
    }

    #[test]
    fn empty_steering_queue_fast_finalizes() {
        let fields = json!({
            "max_follow_ups": "5",
            "follow_up_count": 0,
            "steering_messages": "[]"
        });

        assert!(!should_check_steering(&fields));
    }

    #[test]
    fn missing_or_invalid_steering_queue_fast_finalizes() {
        assert!(!should_check_steering(&json!({"max_follow_ups": "5"})));
        assert!(!should_check_steering(&json!({
            "max_follow_ups": "5",
            "steering_messages": "not-json"
        })));
    }

    #[test]
    fn queued_steering_checks_when_budget_remains() {
        let fields = json!({
            "max_follow_ups": "5",
            "follow_up_count": 1,
            "steering_messages": r#"[{"content":"please revise"}]"#
        });

        assert!(should_check_steering(&fields));
    }

    #[test]
    fn exhausted_steering_budget_fast_finalizes() {
        let fields = json!({
            "max_follow_ups": "2",
            "follow_up_count": 2,
            "steering_messages": r#"[{"content":"too late"}]"#
        });

        assert!(!should_check_steering(&fields));
    }

    #[test]
    fn zero_max_follow_ups_fast_finalizes_even_with_queue() {
        let fields = json!({
            "max_follow_ups": "0",
            "follow_up_count": 0,
            "steering_messages": r#"[{"content":"queued"}]"#
        });

        assert!(!should_check_steering(&fields));
    }

    #[test]
    fn terminal_reply_bypass_only_matches_unrouted_direct_sessions() {
        assert!(should_bypass_terminal_reply("ss-direct", &json!({})));
        assert!(should_bypass_terminal_reply(
            "ss-direct",
            &json!({"agent_id": "ss-direct"})
        ));
        assert!(should_bypass_terminal_reply(
            "ss-direct",
            &json!({
                "agent_id": "aj-direct",
                "reply_route_source": "direct_no_reply"
            })
        ));

        for fields in [
            json!({"reply_channel_id": "discord-channel", "reply_thread_id": "thread"}),
            json!({"reply_thread_id": "thread"}),
            json!({"reply_route_source": "channel_message"}),
            json!({
                "reply_route_source": "direct_no_reply",
                "reply_channel_id": "discord-channel",
                "reply_thread_id": "thread"
            }),
            json!({"parent_session_id": "ss-parent"}),
            json!({"agent_id": "aj-agent"}),
        ] {
            assert!(
                !should_bypass_terminal_reply("ss-direct", &fields),
                "ambiguous or channel-bound fields must keep RecordResult delivery path: {fields}"
            );
        }
    }

    #[test]
    fn inline_reply_route_only_matches_complete_inline_channels() {
        let route = inline_reply_route(
            "ss-inline",
            &json!({
                "reply_channel_id": "cli-channel",
                "reply_channel_entity_id": "en-channel",
                "reply_channel_type": "cli",
                "reply_thread_id": "thread-1",
                "agent_id": "aj-agent"
            }),
        )
        .expect("complete cli route should be eligible");

        assert_eq!(route.channel_id, "cli-channel");
        assert_eq!(route.channel_entity_id, "en-channel");
        assert_eq!(route.channel_type, "cli");
        assert_eq!(route.thread_id, "thread-1");
        assert_eq!(route.agent_entity_id, "aj-agent");

        assert!(inline_reply_route(
            "ss-inline",
            &json!({
                "reply_channel_entity_id": "en-channel",
                "reply_channel_type": "tui",
                "reply_thread_id": "thread-1"
            }),
        )
        .is_some());

        for fields in [
            json!({
                "reply_channel_entity_id": "en-channel",
                "reply_channel_type": "discord",
                "reply_thread_id": "thread-1"
            }),
            json!({
                "reply_channel_type": "cli",
                "reply_thread_id": "thread-1"
            }),
            json!({
                "reply_channel_entity_id": "en-channel",
                "reply_channel_type": "cli"
            }),
        ] {
            assert!(
                inline_reply_route("ss-inline", &fields).is_none(),
                "non-inline or incomplete route must fall back to RecordResult: {fields}"
            );
        }
    }

    #[test]
    fn inline_reply_url_and_body_match_channel_reply_delivered_contract() {
        let route = InlineReplyRoute {
            channel_id: "cli-channel".to_string(),
            channel_entity_id: "en-channel'quoted".to_string(),
            channel_type: "cli".to_string(),
            thread_id: "thread-1".to_string(),
            agent_entity_id: "".to_string(),
        };

        assert_eq!(
            inline_reply_action_url("http://temper", &route.channel_entity_id),
            "http://temper/tdata/Channels('en-channel''quoted')/Paw.Channel.ReplyDelivered"
        );
        assert_eq!(
            inline_reply_body(&route, "hello"),
            json!({
                "thread_id": "thread-1",
                "content": "hello",
                "agent_entity_id": "",
            })
        );
        assert_eq!(
            terminal_phase_name("RecordResultInlineReply"),
            "record_result_inline_reply"
        );
    }

    #[test]
    fn fresh_session_tree_response_apply_does_not_build_legacy_conversation_payload() {
        let prepared = PreparedContextArtifact {
            version: 1,
            messages: vec![json!({"role": "user", "content": "hello"})],
            tools: vec![],
            system_prompt: "You are concise.".to_string(),
            system_prompt_hash: "hash-123".to_string(),
            system_prompt_file_id: "file-system".to_string(),
            conversation_file_id: String::new(),
            session_file_id: "session-file".to_string(),
            session_leaf_id: "leaf-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            use_session_tree: true,
            context_tokens: 12,
            context_bytes: 128,
            entries_loaded: 1,
            content_files_loaded: 0,
            prune_tool_results_after_turns: 4,
        };
        let artifact = ProviderResponseArtifact {
            version: 1,
            provider: "openai_codex".to_string(),
            model: "gpt-5.4-codex".to_string(),
            content: json!([{"type": "tool_use", "id": "call_1", "name": "temper.read", "input": {"path": "/x"}}]),
            stop_reason: "tool_use".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 256,
            response_bytes: 512,
            token_signals: None,
        };

        assert!(legacy_updated_conversation_payload(&prepared, &artifact).is_none());
    }

    #[test]
    fn legacy_inline_response_apply_keeps_conversation_payload() {
        let prepared = PreparedContextArtifact {
            version: 1,
            messages: vec![json!({"role": "user", "content": "hello"})],
            tools: vec![],
            system_prompt: "You are concise.".to_string(),
            system_prompt_hash: "hash-123".to_string(),
            system_prompt_file_id: "file-system".to_string(),
            conversation_file_id: String::new(),
            session_file_id: String::new(),
            session_leaf_id: String::new(),
            workspace_id: String::new(),
            use_session_tree: false,
            context_tokens: 12,
            context_bytes: 128,
            entries_loaded: 1,
            content_files_loaded: 0,
            prune_tool_results_after_turns: 4,
        };
        let artifact = ProviderResponseArtifact {
            version: 1,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            content: json!([{"type": "text", "text": "hi"}]),
            stop_reason: "end_turn".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 256,
            response_bytes: 512,
            token_signals: None,
        };

        let payload = legacy_updated_conversation_payload(&prepared, &artifact)
            .expect("legacy inline mode still needs a conversation param");
        let messages: Vec<Value> = serde_json::from_str(&payload).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "assistant");
    }
}
