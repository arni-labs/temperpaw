//! Session tree persistence, hooks, heartbeat, and file sync.
//!
//! Ported from tool_runner to maintain full compatibility with the
//! agent state machine's conversation and session tracking.

use std::collections::BTreeSet;

use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    append_session_entry_inline, create_content_file_ref, entity_field_str, runtime_headers_as,
    send_typing_indicator, timestamp_millis_string,
};

const SESSION_ENTRY_FILE_THRESHOLD_BYTES: usize = 4096;

/// Persist tool results to the session tree (or conversation file, or inline).
///
/// Returns the params to include in the HandleToolResults callback.
pub fn persist_results(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    tool_results: &[Value],
    repl_file_id: &str,
) -> Result<Value, String> {
    let conversation_file_id = field_str(fields, "conversation_file_id");
    let session_file_id = field_str(fields, "session_file_id");
    let session_leaf_id = field_str(fields, "session_leaf_id");
    let workspace_id = field_str(fields, "workspace_id");

    let results_json = serde_json::to_string(tool_results).unwrap_or_default();
    // Store repl_file_id (pointer to TemperFS file), not the full repl_state blob
    let mut params = json!({
        "pending_tool_calls": results_json,
        "repl_file_id": repl_file_id,
    });
    let reply_attachments_json = reply_attachments_from_tool_results(tool_results);
    if !reply_attachments_json.is_empty() {
        params["reply_attachments_json"] = json!(reply_attachments_json);
    }

    if !session_file_id.is_empty() && !session_leaf_id.is_empty() {
        // Session tree mode
        let entity_backed_session = wasm_helpers::is_session_entries_ref(session_file_id);
        if entity_backed_session {
            let tool_results_value = json!(tool_results);
            let tokens_est = results_json.len() / 4;
            ctx.log(
                "info",
                &format!(
                    "monty_repl.persist_results: writing tool_result entry parent={session_leaf_id} session_file={session_file_id} tokens_est={tokens_est} count={}",
                    tool_results.len()
                ),
            );
            let created = append_session_entry_inline(
                ctx,
                temper_api_url,
                tenant,
                fields,
                session_file_id,
                session_leaf_id,
                "t",
                "user",
                &tool_results_value,
                tokens_est,
                None,
            )?;
            // create_session_entry now does a read-back verify, so reaching
            // here means the row is durably visible.
            ctx.log(
                "info",
                &format!(
                    "monty_repl.persist_results: append OK entry_id={} entity_id={}",
                    created.entry_id, created.entity_id
                ),
            );

            params["pending_tool_calls"] = json!(compact_tool_results_marker(tool_results));
            params["session_leaf_id"] = json!(created.entry_id);
            return Ok(params);
        }

        let session_jsonl = if entity_backed_session {
            wasm_helpers::read_session_from_temperfs(
                ctx,
                temper_api_url,
                tenant,
                fields,
                session_file_id,
            )?
        } else {
            read_temperfs_file(ctx, temper_api_url, tenant, session_file_id)?
        };
        let mut tree = session_tree_lib::SessionTree::from_jsonl(&session_jsonl);
        let tool_results_value = json!(tool_results);
        let tokens_est = results_json.len() / 4;
        let content_str = serde_json::to_string(&tool_results_value).unwrap_or_default();

        let (new_leaf, _) = if !entity_backed_session
            && !workspace_id.is_empty()
            && should_store_entry_as_file(&content_str)
        {
            match create_content_file_ref(
                ctx,
                temper_api_url,
                tenant,
                workspace_id,
                &format!("t-{}", tree.len()),
                &content_str,
            ) {
                Ok(content_ref) => tree.append_tool_results_file(
                    session_leaf_id,
                    &content_ref.file_id,
                    Some(&content_ref.file_version_id),
                    tokens_est,
                ),
                Err(_) => {
                    tree.append_tool_results(session_leaf_id, &tool_results_value, tokens_est)
                }
            }
        } else {
            tree.append_tool_results(session_leaf_id, &tool_results_value, tokens_est)
        };

        let updated_jsonl = tree.to_jsonl();
        if entity_backed_session {
            wasm_helpers::write_session_to_temperfs(
                ctx,
                temper_api_url,
                tenant,
                fields,
                session_file_id,
                &updated_jsonl,
            )?;
        } else {
            write_temperfs_file(ctx, temper_api_url, tenant, session_file_id, &updated_jsonl)?;
        }

        params["pending_tool_calls"] = json!(compact_tool_results_marker(tool_results));
        params["session_leaf_id"] = json!(new_leaf);
    } else if !conversation_file_id.is_empty() {
        // Legacy flat JSON mode
        let mut messages: Vec<Value> =
            read_conversation(ctx, temper_api_url, tenant, conversation_file_id)?;
        messages.push(json!({ "role": "user", "content": tool_results }));

        let updated = serde_json::to_string(&messages).unwrap_or_default();
        let body = format!("{{\"messages\":{updated}}}");
        let url = format!("{temper_api_url}/tdata/Files('{conversation_file_id}')/$value");
        let headers = file_headers(ctx, tenant);
        let resp = ctx.http_call("PUT", &url, &headers, &body)?;
        if resp.status >= 400 {
            return Err(format!(
                "TemperFS conversation write failed (HTTP {})",
                resp.status
            ));
        }
        params["conversation"] = json!(updated);
    } else {
        // Inline mode
        let mut messages: Vec<Value> = {
            let conv = fields
                .get("conversation")
                .and_then(|v| v.as_str())
                .unwrap_or("[]");
            serde_json::from_str(conv).unwrap_or_default()
        };
        messages.push(json!({ "role": "user", "content": tool_results }));
        let updated = serde_json::to_string(&messages).unwrap_or_default();
        params["conversation"] = json!(updated);
    }

    Ok(params)
}

pub fn reply_attachments_from_tool_results(tool_results: &[Value]) -> String {
    let mut seen = BTreeSet::new();
    let mut attachments = Vec::new();

    for result in tool_results {
        collect_reply_attachment_from_value(
            result.get("content").unwrap_or(result),
            &mut seen,
            &mut attachments,
        );
    }

    if attachments.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&attachments).unwrap_or_default()
    }
}

fn collect_reply_attachment_from_value(
    value: &Value,
    seen: &mut BTreeSet<String>,
    attachments: &mut Vec<Value>,
) {
    if let Some(items) = value.as_array() {
        for item in items {
            collect_reply_attachment_from_value(item, seen, attachments);
        }
        return;
    }

    if let Some(text) = value.as_str()
        && let Ok(parsed) = serde_json::from_str::<Value>(text)
    {
        collect_reply_attachment_from_value(&parsed, seen, attachments);
        return;
    }

    let Some(object) = value.as_object() else {
        return;
    };

    if object
        .get("__temperpaw_image")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "image")
    {
        let file_id = object
            .get("file_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if file_id.is_empty() || !seen.insert(file_id.to_string()) {
            return;
        }
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let mime_type = object
            .get("mime_type")
            .or_else(|| object.get("media_type"))
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("source")
                    .and_then(|source| source.get("media_type"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("image/png");
        attachments.push(json!({
            "kind": "pawfs_file",
            "file_id": file_id,
            "file_version_id": object.get("file_version_id").and_then(Value::as_str).unwrap_or(""),
            "filename": attachment_filename(path, mime_type),
            "mime_type": mime_type,
            "path": path,
            "media_generation_id": object.get("media_generation_id").and_then(Value::as_str).unwrap_or(""),
        }));
    }
}

fn attachment_filename(path: &str, mime_type: &str) -> String {
    let from_path = path
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(filename) = from_path {
        return filename.to_string();
    }

    match mime_type {
        "image/jpeg" | "image/jpg" => "image.jpg".to_string(),
        "image/webp" => "image.webp".to_string(),
        _ => "image.png".to_string(),
    }
}

/// Send a liveness heartbeat and post the Discord typing indicator on the
/// same cadence.
///
/// `Heartbeat` is a pure liveness ping — it records `last_heartbeat_at` and
/// drives the typing indicator, but it does NOT reset any state_timeout. Use
/// this when the module hasn't actually advanced (e.g., about to block on an
/// HTTP call). For real forward progress (tool batch completed, chunk
/// received), call `send_progress` instead.
pub fn send_heartbeat(ctx: &Context, temper_api_url: &str, tenant: &str) {
    post_session_progress_signal(
        ctx,
        temper_api_url,
        tenant,
        "Heartbeat",
        "last_heartbeat_at",
        "heartbeat_dispatch_enabled",
    );
}

/// Signal real forward progress: dispatches `ProgressMade` (resets the
/// current state_timeout) and posts the Discord typing indicator.
///
/// Call this when the module has actually advanced — e.g., after completing
/// a tool batch in monty_repl, or when llm_caller has received the first
/// streamed chunk. Contrast with `send_heartbeat`, which only signals
/// liveness and does not reset timeouts.
pub fn send_progress(ctx: &Context, temper_api_url: &str, tenant: &str) {
    post_session_progress_signal(
        ctx,
        temper_api_url,
        tenant,
        "ProgressMade",
        "last_progress_at",
        "tool_progress_dispatch_enabled",
    );
}

fn post_session_progress_signal(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    action: &str,
    timestamp_field: &str,
    enabled_key: &str,
) {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if progress_action_dispatch_enabled(ctx, &fields, enabled_key) {
        let url = format!(
            "{temper_api_url}/tdata/Sessions('{}')/TemperPaw.{action}",
            ctx.entity_id
        );
        let body = json!({ timestamp_field: timestamp_millis_string() });
        let headers = runtime_headers_as(
            ctx,
            tenant,
            &fields,
            "system",
            Some("application/json"),
            None,
        );
        let _ = ctx.http_call("POST", &url, &headers, &body.to_string());
    }

    // Post typing indicator directly (replaces the old `heartbeat_typing`
    // trigger cascade). Uses the persistent Agent entity ID from session
    // fields, falling back to the session's own id — same resolution the
    // deleted `heartbeat_typing` WASM module used.
    let typing_agent_id =
        entity_field_str(&fields, &["agent_id", "AgentId"]).unwrap_or(&ctx.entity_id);
    send_typing_indicator(ctx, temper_api_url, tenant, typing_agent_id);
}

fn progress_action_dispatch_enabled(ctx: &Context, fields: &Value, key: &str) -> bool {
    fields
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| ctx.config.get(key).map(String::as_str))
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Largest `result` kept in a persisted tool span. The OTS decision only ever
/// shows the first 500 characters; the full result already lives in the
/// tool_result session entry, so a longer copy here buys nothing and the span
/// file is rewritten in full on every tool batch.
const TOOL_SPAN_RESULT_MAX_CHARS: usize = 600;
/// Largest `arguments` payload kept in a persisted tool span.
const TOOL_SPAN_ARGUMENTS_MAX_CHARS: usize = 2_000;
/// Ceiling on the whole tool-span document. Past this the session stops
/// appending rather than paying an unbounded read-modify-write per tool batch.
const TOOL_SPANS_FILE_MAX_BYTES: usize = 262_144;
/// Reserved tool name marking a span document that hit the ceiling. The OTS
/// emitter recognizes it and records the truncation instead of turning it into
/// a decision.
pub const TOOL_SPANS_TRUNCATED_MARKER: &str = "_tool_spans_truncated";
/// The marker line, written once when the document seals.
const TOOL_SPANS_TRUNCATED_LINE: &str = "{\"tool_call_id\":\"\",\"tool_name\":\"_tool_spans_truncated\",\"result\":\"tool span file size ceiling reached\",\"duration_ms\":0,\"is_error\":false}\n";

/// True once the document has been sealed by a truncation marker.
///
/// Only the last non-empty line is examined, and only its `tool_name` field:
/// the marker is a reserved tool name, so a substring search would also fire on
/// a tool result that merely quotes it. Byte slicing is avoided outright — a
/// result carrying CJK or emoji puts arbitrary offsets inside a character.
fn tool_spans_document_sealed(document: &str) -> bool {
    document
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(is_tool_spans_truncation_line)
}

/// True when a JSONL line is the reserved truncation marker record.
fn is_tool_spans_truncation_line(line: &str) -> bool {
    serde_json::from_str::<Value>(line.trim())
        .ok()
        .as_ref()
        .and_then(|value| value.get("tool_name"))
        .and_then(Value::as_str)
        == Some(TOOL_SPANS_TRUNCATED_MARKER)
}

fn truncate_span_chars(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars).collect();
    out.push_str(&format!("...[truncated {} of {total} chars]", total - max_chars));
    out
}

/// Shrink a tool-span event to what the trajectory emitter actually consumes.
pub fn compact_tool_span(event: &Value) -> Value {
    let mut compacted = event.clone();
    if let Some(object) = compacted.as_object_mut() {
        if let Some(Value::String(result)) = object.get("result") {
            let bounded = truncate_span_chars(result, TOOL_SPAN_RESULT_MAX_CHARS);
            object.insert("result".to_string(), json!(bounded));
        }
        if let Some(Value::String(arguments)) = object.get("arguments") {
            let bounded = truncate_span_chars(arguments, TOOL_SPAN_ARGUMENTS_MAX_CHARS);
            object.insert("arguments".to_string(), json!(bounded));
        }
    }
    compacted
}

/// Encode a tool-span JSONL document by appending new events to the existing content.
///
/// Each event in `new_events` is serialized as a single JSON object on its own line,
/// separated by '\n'. The returned string always ends with '\n' so that subsequent
/// appends stay line-delimited. Events are compacted first, and appends stop once
/// the document reaches `TOOL_SPANS_FILE_MAX_BYTES` so a long session cannot turn
/// the per-batch rewrite into unbounded traffic.
pub fn encode_tool_spans_jsonl(existing: &str, new_events: &[Value]) -> String {
    let mut out = String::with_capacity(existing.len() + new_events.len() * 128);
    if !existing.is_empty() {
        out.push_str(existing);
        if !existing.ends_with('\n') {
            out.push('\n');
        }
    }
    if tool_spans_document_sealed(&out) {
        return out;
    }
    if out.len() >= TOOL_SPANS_FILE_MAX_BYTES {
        // At the ceiling with no seal on it: a document written before this
        // ceiling existed, or a batch that landed byte-exact on it. The spans
        // in hand are being dropped, so the document has to say so — otherwise
        // it parses clean and the trajectory built from it reads as holding
        // every tool call the session made. Sealing costs one line over the
        // ceiling, which is bounded; silence costs the record.
        if !new_events.is_empty() {
            out.push_str(TOOL_SPANS_TRUNCATED_LINE);
        }
        return out;
    }
    for event in new_events {
        let line = serde_json::to_string(&compact_tool_span(event))
            .unwrap_or_else(|_| "{}".to_string());
        if out.len() + line.len() + 1 > TOOL_SPANS_FILE_MAX_BYTES {
            out.push_str(TOOL_SPANS_TRUNCATED_LINE);
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Append tool-span JSONL events to the session's tool_spans_file_id file.
///
/// Creates the file on first call (when `existing_file_id` is empty). Reads existing
/// JSONL, appends new events, writes back. Returns the file id on success. Returns
/// an empty string when `workspace_id` is empty (no workspace — spans are not persisted).
pub fn append_tool_spans_to_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    existing_file_id: &str,
    new_events: &[Value],
) -> Result<String, String> {
    if workspace_id.is_empty() || new_events.is_empty() {
        return Ok(existing_file_id.to_string());
    }

    let file_id = if existing_file_id.is_empty() {
        let body = json!({
            "Name": "tool_spans.jsonl",
            "path": "/tool_spans.jsonl",
            "WorkspaceId": workspace_id,
        });
        let url = format!("{temper_api_url}/tdata/Files");
        let headers = file_headers(ctx, tenant);
        let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
        if resp.status >= 400 {
            return Err(format!(
                "failed to create tool_spans file (status={}): {}",
                resp.status, resp.body
            ));
        }
        let created: Value = serde_json::from_str(&resp.body)
            .map_err(|e| format!("failed to parse file create response: {e}"))?;
        created
            .get("entity_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        existing_file_id.to_string()
    };

    if file_id.is_empty() {
        return Ok(String::new());
    }

    let existing_content = read_temperfs_file_safe(ctx, temper_api_url, tenant, &file_id)?;
    let updated = encode_tool_spans_jsonl(&existing_content, new_events);

    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = workspace_headers(ctx, tenant, workspace_id);
    write_temperfs_file_with_retry(
        ctx,
        &url,
        &headers,
        &updated,
        "failed to write tool_spans file",
    )?;

    Ok(file_id)
}

/// Save REPL state to a TemperFS file. Creates the file on first call,
/// overwrites on subsequent calls. Returns the file ID.
pub fn save_repl_to_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    workspace_id: &str,
    existing_file_id: &str,
    repl_state_b64: &str,
) -> Result<String, String> {
    if workspace_id.is_empty() {
        // No workspace — fall back to storing nothing (fresh REPL each turn)
        return Ok(String::new());
    }

    let file_id = if existing_file_id.is_empty() {
        // Create a new file
        let body = json!({
            "Name": "repl_state.b64",
            "path": "/repl_state.b64",
            "WorkspaceId": workspace_id,
        });
        let url = format!("{temper_api_url}/tdata/Files");
        let headers = file_headers(ctx, tenant);
        let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
        if resp.status >= 400 {
            return Err(format!(
                "failed to create REPL state file (status={}): {}",
                resp.status, resp.body
            ));
        }
        let created: Value = serde_json::from_str(&resp.body)
            .map_err(|e| format!("failed to parse file create response: {e}"))?;
        created
            .get("entity_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        existing_file_id.to_string()
    };

    if file_id.is_empty() {
        return Ok(String::new());
    }

    // Write REPL state to file
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = workspace_headers(ctx, tenant, workspace_id);
    write_temperfs_file_with_retry(
        ctx,
        &url,
        &headers,
        repl_state_b64,
        "failed to write REPL state file",
    )?;

    Ok(file_id)
}

/// Read a TemperFS file safely (returns empty string on error).
pub fn read_temperfs_file_safe(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    file_id: &str,
) -> Result<String, String> {
    if file_id.is_empty() {
        return Ok(String::new());
    }
    read_temperfs_file(ctx, temper_api_url, tenant, file_id)
}

fn workspace_headers(ctx: &Context, tenant: &str, workspace_id: &str) -> Vec<(String, String)> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut headers = runtime_headers_as(ctx, tenant, &fields, "system", Some("text/plain"), None);
    headers.push(("X-Workspace-Id".to_string(), workspace_id.to_string()));
    headers
}

/// Evaluate before-hooks for a tool/op name. Returns error string if blocked.
/// Not yet wired into the dispatch pipeline; will be called once hook policies are enabled.
#[allow(dead_code)]
pub fn evaluate_before_hooks(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    soul_id: &str,
    hook_policy: &str,
    op_name: &str,
) -> Result<Option<String>, String> {
    if hook_policy == "none" || soul_id.is_empty() {
        return Ok(None);
    }
    let hooks = load_matching_hooks(ctx, temper_api_url, tenant, soul_id, "before", op_name)?;
    for hook in hooks {
        let action = entity_field_str(&hook, &["HookAction"]).unwrap_or("log");
        let name = entity_field_str(&hook, &["Name"]).unwrap_or("hook");
        match action {
            "block" => {
                return Ok(Some(format!(
                    "operation blocked by hook '{name}' for '{op_name}'"
                )));
            }
            "log" => ctx.log(
                "info",
                &format!("monty_repl: before hook '{name}' matched {op_name}"),
            ),
            _ => {}
        }
    }
    Ok(None)
}

/// Apply after-hooks to tool output.
/// Not yet wired into the dispatch pipeline; will be called once hook policies are enabled.
#[allow(dead_code)]
pub fn apply_after_hooks(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    soul_id: &str,
    hook_policy: &str,
    op_name: &str,
    mut output: String,
) -> Result<String, String> {
    if hook_policy != "full_hooks" || soul_id.is_empty() {
        return Ok(output);
    }
    let hooks = load_matching_hooks(ctx, temper_api_url, tenant, soul_id, "after", op_name)?;
    for hook in hooks {
        let action = entity_field_str(&hook, &["HookAction"]).unwrap_or("log");
        let name = entity_field_str(&hook, &["Name"]).unwrap_or("hook");
        match action {
            "modify" => output = format!("[modified by hook:{name}]\n{output}"),
            "log" => ctx.log(
                "info",
                &format!("monty_repl: after hook '{name}' matched {op_name}"),
            ),
            _ => {}
        }
    }
    Ok(output)
}

// --- Internal helpers ---

#[allow(dead_code)]
fn load_matching_hooks(
    ctx: &Context,
    temper_api_url: &str,
    _tenant: &str,
    soul_id: &str,
    hook_type: &str,
    op_name: &str,
) -> Result<Vec<Value>, String> {
    let url = format!("{temper_api_url}/tdata/ToolHooks");
    let headers = internal_headers();
    let resp = ctx.http_call("GET", &url, &headers, "")?;
    if resp.status != 200 {
        return Ok(Vec::new());
    }
    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({ "value": [] }));
    let hooks = parsed
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|hook| {
            entity_field_str(hook, &["Status"]) == Some("Active")
                && entity_field_str(hook, &["SoulId"]).unwrap_or("") == soul_id
                && entity_field_str(hook, &["HookType"]).unwrap_or("") == hook_type
                && hook_matches(
                    entity_field_str(hook, &["ToolPattern"]).unwrap_or(".*"),
                    op_name,
                )
        })
        .collect();
    Ok(hooks)
}

#[allow(dead_code)]
fn hook_matches(pattern: &str, op_name: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern == ".*" || pattern == "*" {
        return true;
    }
    if pattern.contains('|') {
        return pattern.split('|').any(|part| part.trim() == op_name);
    }
    pattern == op_name
}

fn compact_tool_results_marker(tool_results: &[Value]) -> String {
    let markers: Vec<String> = tool_results
        .iter()
        .map(|r| {
            let tool_id = r.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("?");
            let is_error = r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            if is_error {
                format!("{{\"tool_use_id\":\"{tool_id}\",\"is_error\":true}}")
            } else {
                format!("{{\"tool_use_id\":\"{tool_id}\"}}")
            }
        })
        .collect();
    format!("[{}]", markers.join(","))
}

fn read_temperfs_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    file_id: &str,
) -> Result<String, String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = file_headers(ctx, tenant);

    const READ_ATTEMPTS: usize = 5;
    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..READ_ATTEMPTS {
        let resp = ctx.http_call("GET", &url, &headers, "")?;
        if resp.status == 200 {
            return Ok(resp.body);
        }
        if resp.status == 404 {
            return Ok(String::new());
        }

        last_status = resp.status;
        last_body = resp.body;

        if (500..600).contains(&last_status) && attempt + 1 < READ_ATTEMPTS {
            ctx.log(
                "warn",
                &format!(
                    "monty_repl: TemperFS read transient HTTP {}, retry {}/{}",
                    last_status,
                    attempt + 2,
                    READ_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "TemperFS read failed (HTTP {}): {}",
        last_status,
        &last_body[..last_body.len().min(200)]
    ))
}

fn is_retriable_write_failure(status: u16, body: &str) -> bool {
    (500..600).contains(&status) || body.contains("BlobUploadFailed") || body.contains("HTTP -1")
}

fn write_temperfs_file_with_retry(
    ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    content: &str,
    label: &str,
) -> Result<(), String> {
    const WRITE_ATTEMPTS: usize = 5;
    let mut last_status = 0;
    let mut last_body = String::new();

    for attempt in 0..WRITE_ATTEMPTS {
        let resp = ctx.http_call("PUT", url, headers, content)?;
        if (200..300).contains(&resp.status) {
            return Ok(());
        }

        last_status = resp.status;
        last_body = resp.body;

        if is_retriable_write_failure(last_status, &last_body) && attempt + 1 < WRITE_ATTEMPTS {
            ctx.log(
                "warn",
                &format!(
                    "monty_repl: {label} transient HTTP {}, retry {}/{}",
                    last_status,
                    attempt + 2,
                    WRITE_ATTEMPTS
                ),
            );
            continue;
        }
        break;
    }

    Err(format!(
        "{label} (HTTP {}): {}",
        last_status,
        &last_body[..last_body.len().min(200)]
    ))
}

fn write_temperfs_file(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    file_id: &str,
    content: &str,
) -> Result<(), String> {
    let url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let headers = file_headers_text(ctx, tenant);
    write_temperfs_file_with_retry(ctx, &url, &headers, content, "TemperFS write failed")
}

fn read_conversation(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    file_id: &str,
) -> Result<Vec<Value>, String> {
    let content = read_temperfs_file(ctx, temper_api_url, tenant, file_id)?;
    if content.is_empty() {
        return Ok(Vec::new());
    }
    // Try parsing as {messages: [...]} or bare [...]
    if let Ok(wrapper) = serde_json::from_str::<Value>(&content) {
        if let Some(msgs) = wrapper.get("messages").and_then(Value::as_array) {
            return Ok(msgs.clone());
        }
    }
    serde_json::from_str(&content).map_err(|e| format!("failed to parse conversation: {e}"))
}

fn field_str<'a>(fields: &'a Value, key: &str) -> &'a str {
    fields.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn should_store_entry_as_file(content: &str) -> bool {
    content.len() > SESSION_ENTRY_FILE_THRESHOLD_BYTES
}

fn internal_headers() -> Vec<(String, String)> {
    vec![("Content-Type".to_string(), "application/json".to_string())]
}

fn file_headers(ctx: &Context, tenant: &str) -> Vec<(String, String)> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    runtime_headers_as(
        ctx,
        tenant,
        &fields,
        "system",
        Some("application/json"),
        Some("application/octet-stream"),
    )
}

fn file_headers_text(ctx: &Context, tenant: &str) -> Vec<(String, String)> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    runtime_headers_as(ctx, tenant, &fields, "system", Some("text/plain"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_tool_spans_jsonl_creates_three_lines_from_empty() {
        let events = vec![
            json!({"tool_call_id": "a", "tool_name": "read", "duration_ms": 12}),
            json!({"tool_call_id": "b", "tool_name": "write", "duration_ms": 45}),
            json!({"tool_call_id": "c", "tool_name": "bash", "duration_ms": 78}),
        ];
        let out = encode_tool_spans_jsonl("", &events);
        let lines: Vec<&str> = out.split_terminator('\n').collect();
        assert_eq!(
            lines.len(),
            3,
            "expected 3 JSONL lines, got {}\n{}",
            lines.len(),
            out
        );
        for (i, line) in lines.iter().enumerate() {
            let parsed: Value = serde_json::from_str(line).expect("each line must be valid JSON");
            assert_eq!(parsed["tool_call_id"], events[i]["tool_call_id"]);
        }
        assert!(out.ends_with('\n'), "output must end with newline");
    }

    #[test]
    fn encode_tool_spans_jsonl_appends_to_existing() {
        let existing = "{\"tool_call_id\":\"prev\"}\n";
        let new_events = vec![json!({"tool_call_id": "next"})];
        let out = encode_tool_spans_jsonl(existing, &new_events);
        let lines: Vec<&str> = out.split_terminator('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            serde_json::from_str::<Value>(lines[0]).unwrap()["tool_call_id"],
            "prev"
        );
        assert_eq!(
            serde_json::from_str::<Value>(lines[1]).unwrap()["tool_call_id"],
            "next"
        );
    }

    #[test]
    fn encode_tool_spans_jsonl_handles_existing_without_trailing_newline() {
        let existing = "{\"tool_call_id\":\"prev\"}";
        let new_events = vec![json!({"tool_call_id": "next"})];
        let out = encode_tool_spans_jsonl(existing, &new_events);
        assert!(
            out.contains("{\"tool_call_id\":\"prev\"}\n{\"tool_call_id\":\"next\"}"),
            "missing newline separator between existing content and new event: {out}"
        );
    }

    #[test]
    fn encode_tool_spans_jsonl_empty_events_returns_existing() {
        let existing = "{\"tool_call_id\":\"prev\"}\n";
        let out = encode_tool_spans_jsonl(existing, &[]);
        assert_eq!(out, existing);
    }

    #[test]
    fn compact_tool_span_bounds_result_and_arguments() {
        let event = json!({
            "tool_call_id": "a",
            "tool_name": "temper.bash",
            "arguments": "x".repeat(TOOL_SPAN_ARGUMENTS_MAX_CHARS + 500),
            "result": "y".repeat(TOOL_SPAN_RESULT_MAX_CHARS + 5_000),
            "duration_ms": 12,
            "is_error": false,
        });
        let compacted = compact_tool_span(&event);
        assert_eq!(compacted["tool_call_id"], "a");
        assert_eq!(compacted["duration_ms"], 12);
        let result = compacted["result"].as_str().unwrap();
        assert!(result.starts_with(&"y".repeat(TOOL_SPAN_RESULT_MAX_CHARS)));
        assert!(result.contains("truncated 5000 of"));
        let arguments = compacted["arguments"].as_str().unwrap();
        assert!(arguments.contains("truncated 500 of"));
    }

    #[test]
    fn compact_tool_span_leaves_small_events_untouched() {
        let event = json!({
            "tool_call_id": "a",
            "tool_name": "read",
            "arguments": "{\"path\":\"/tmp/x\"}",
            "result": "ok",
            "duration_ms": 3,
            "is_error": false,
        });
        assert_eq!(compact_tool_span(&event), event);
    }

    #[test]
    fn encode_tool_spans_jsonl_stops_at_the_size_ceiling() {
        let event = json!({
            "tool_call_id": "a",
            "tool_name": "temper.bash",
            "arguments": "z".repeat(TOOL_SPAN_ARGUMENTS_MAX_CHARS),
            "result": "w".repeat(TOOL_SPAN_RESULT_MAX_CHARS),
            "duration_ms": 1,
            "is_error": false,
        });
        let events: Vec<Value> = std::iter::repeat_n(event, 400).collect();
        let out = encode_tool_spans_jsonl("", &events);
        assert!(
            out.len() <= TOOL_SPANS_FILE_MAX_BYTES,
            "span file must stay bounded, got {} bytes",
            out.len()
        );
        assert!(out.contains("_tool_spans_truncated"));
        // Every line still parses — a truncated document must stay readable.
        for line in out.split_terminator('\n') {
            serde_json::from_str::<Value>(line).expect("each line must be valid JSON");
        }
    }

    #[test]
    fn encode_tool_spans_jsonl_seals_a_full_document_it_refuses_to_grow() {
        let existing = format!("{}\n", "x".repeat(TOOL_SPANS_FILE_MAX_BYTES));
        let out = encode_tool_spans_jsonl(&existing, &[json!({"tool_call_id": "next"})]);

        assert!(
            out.starts_with(&existing),
            "a full document must not grow with new spans"
        );
        assert!(
            !out.contains("\"tool_call_id\":\"next\""),
            "the refused span must not be written"
        );
        assert!(
            tool_spans_document_sealed(&out),
            "a document that is dropping spans must say so; a clean parse of it \
             otherwise reads as a complete record of the session's tool calls"
        );

        // Sealing is idempotent — later batches find the seal and stop.
        let again = encode_tool_spans_jsonl(&out, &[json!({"tool_call_id": "later"})]);
        assert_eq!(again, out, "an already sealed document must not grow");
        assert_eq!(
            again.matches(TOOL_SPANS_TRUNCATED_MARKER).count(),
            1,
            "the seal is written once"
        );
    }

    #[test]
    fn encode_tool_spans_jsonl_does_not_seal_a_full_document_with_nothing_to_add() {
        let existing = format!("{}\n", "x".repeat(TOOL_SPANS_FILE_MAX_BYTES));
        let out = encode_tool_spans_jsonl(&existing, &[]);
        assert_eq!(
            out, existing,
            "no spans were dropped, so nothing was lost and nothing is marked"
        );
    }

    #[test]
    fn encode_tool_spans_jsonl_seals_the_document_once() {
        let event = json!({
            "tool_call_id": "a",
            "tool_name": "temper.bash",
            "arguments": "z".repeat(TOOL_SPAN_ARGUMENTS_MAX_CHARS),
            "result": "w".repeat(TOOL_SPAN_RESULT_MAX_CHARS),
            "duration_ms": 1,
            "is_error": false,
        });
        let events: Vec<Value> = std::iter::repeat_n(event.clone(), 400).collect();
        let sealed = encode_tool_spans_jsonl("", &events);
        let markers = sealed.matches(TOOL_SPANS_TRUNCATED_MARKER).count();
        assert_eq!(markers, 1);

        // A later batch must not stack a second marker onto a sealed document.
        let again = encode_tool_spans_jsonl(&sealed, &[event]);
        assert_eq!(again, sealed);
        assert_eq!(again.matches(TOOL_SPANS_TRUNCATED_MARKER).count(), 1);
    }

    /// Tool output is arbitrary UTF-8. The seal check used to slice the document
    /// at a byte offset, which traps the guest mid-character on any multibyte
    /// tail — after the tools already ran, so the result callback never fires.
    #[test]
    fn encode_tool_spans_jsonl_survives_multibyte_tool_output() {
        let existing = format!(
            "{}\n",
            json!({
                "tool_call_id": "a",
                "tool_name": "temper.bash",
                "result": "日本語のテキスト🎌".repeat(20),
                "duration_ms": 1,
                "is_error": false,
            })
        );
        let out = encode_tool_spans_jsonl(&existing, &[json!({"tool_call_id": "b"})]);
        assert!(out.starts_with(&existing));
        assert!(out.contains("\"tool_call_id\":\"b\""));
    }

    /// A tool that reads or greps this source returns the marker literal in its
    /// own output. That must not look like a sealed document.
    #[test]
    fn encode_tool_spans_jsonl_ignores_a_quoted_marker_in_tool_output() {
        let existing = format!(
            "{}\n",
            json!({
                "tool_call_id": "a",
                "tool_name": "temper.read",
                "result": format!("const MARKER = \"{TOOL_SPANS_TRUNCATED_MARKER}\";"),
                "duration_ms": 1,
                "is_error": false,
            })
        );
        let out = encode_tool_spans_jsonl(&existing, &[json!({"tool_call_id": "b"})]);
        assert!(
            out.contains("\"tool_call_id\":\"b\""),
            "a quoted marker must not seal the document"
        );
    }

    #[test]
    fn tool_spans_document_sealed_reads_the_marker_record_only() {
        assert!(!tool_spans_document_sealed(""));
        assert!(tool_spans_document_sealed(TOOL_SPANS_TRUNCATED_LINE));
        assert!(
            !tool_spans_document_sealed(
                "{\"tool_call_id\":\"a\",\"tool_name\":\"read\",\"result\":\"_tool_spans_truncated\"}\n"
            ),
            "the marker is a reserved tool_name, not a substring of any field"
        );
    }
}
