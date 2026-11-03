//! Context Compactor — WASM module for compacting long agent conversations.
//!
//! When the session tree exceeds the context window (minus reserve_tokens),
//! this module is triggered. It summarizes older messages using an LLM call
//! and replaces them with a compaction entry in the session tree.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_get_context(_buf_ptr: i32, _buf_len: i32) -> i32 {
    -1
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_http_call(
    _method_ptr: i32,
    _method_len: i32,
    _url_ptr: i32,
    _url_len: i32,
    _headers_ptr: i32,
    _headers_len: i32,
    _body_ptr: i32,
    _body_len: i32,
    _result_buf_ptr: i32,
    _result_buf_len: i32,
) -> i32 {
    -1
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_log(_level_ptr: i32, _level_len: i32, _msg_ptr: i32, _msg_len: i32) {}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_set_result(_ptr: i32, _len: i32) {}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_get_time_millis() -> i64 {
    0
}

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_read_field(
    _field_name_ptr: i32,
    _field_name_len: i32,
    _buf_ptr: i32,
    _buf_len: i32,
) -> i32 {
    -1
}

use openai_codex_wire::{
    build_openai_headers, extract_chatgpt_account_id_from_jwt, is_openai_codex_token_expired_error,
};
use openai_chat_wire::{
    build_chat_completion_body, parse_chat_completion_response_text, parse_headers_json,
};
use session_tree_lib::SessionTree;
use std::collections::BTreeSet;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    create_content_file_ref, is_session_entries_ref, read_content_file, read_content_file_version,
    read_session_from_temperfs, read_text_file_versions_batch, read_text_files_batch,
    resolve_temper_api_url, runtime_headers, write_session_to_temperfs,
};

const COMPACTION_AUTH_EXPIRED_PREFIX: &str = "compaction_auth_expired:";
const LARGE_SESSION_TREE_SKIP_ENTRIES: usize = 900;

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        ctx.log("info", "context_compactor: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        // Read compaction parameters
        let keep_recent_tokens: usize = fields
            .get("keep_recent_tokens")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(10000);

        let session_file_id = fields
            .get("session_file_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let session_leaf_id = fields
            .get("session_leaf_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if session_file_id.is_empty() || session_leaf_id.is_empty() {
            return Err(
                "context_compactor: missing session_file_id or session_leaf_id".to_string(),
            );
        }

        if should_skip_repeated_compaction(&fields, session_leaf_id) {
            let context_tokens = read_usize_field(&fields, "context_tokens")
                .or_else(|| read_usize_field(&fields, "input_tokens"))
                .unwrap_or(0);
            ctx.log(
                "warn",
                &format!(
                    "context_compactor: repeated compaction guard skipping compaction for leaf {session_leaf_id}"
                ),
            );
            set_compaction_skipped_success_result(
                &fields,
                session_leaf_id,
                context_tokens,
                "repeated_compaction_guard",
            );
            return Ok(());
        }

        let temper_api_url = resolve_temper_api_url(&ctx, &fields);
        let tenant = &ctx.tenant;
        let workspace_id = fields
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 1. Read session tree from TemperFS
        let session_jsonl =
            read_session_from_temperfs(&ctx, &temper_api_url, tenant, &fields, session_file_id)?;
        let mut tree = SessionTree::from_jsonl(&session_jsonl);

        ctx.log(
            "info",
            &format!(
                "context_compactor: tree has {} entries, estimating tokens from leaf {}",
                tree.len(),
                session_leaf_id
            ),
        );

        if tree.len() >= LARGE_SESSION_TREE_SKIP_ENTRIES {
            ctx.log(
                "warn",
                &format!(
                    "context_compactor: large session tree has {} entries; skipping compaction for leaf {session_leaf_id}",
                    tree.len()
                ),
            );
            set_compaction_skipped_success_result(
                &fields,
                session_leaf_id,
                tree.estimate_tokens(session_leaf_id),
                "large_session_tree_guard",
            );
            return Ok(());
        }

        // 2. Find cut point
        let cut_point = match tree.find_cut_point(session_leaf_id, keep_recent_tokens) {
            Some(cp) => cp,
            None => {
                ctx.log(
                    "warn",
                    "context_compactor: no valid cut point found, skipping compaction",
                );
                set_compaction_skipped_result(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    &fields,
                    session_leaf_id,
                    tree.estimate_tokens(session_leaf_id),
                    "no_valid_cut_point",
                );
                return Ok(());
            }
        };

        ctx.log(
            "info",
            &format!("context_compactor: cut point at entry {}", cut_point),
        );

        // 3. Build compaction prompt from messages being cut
        let context_refs = tree.build_context_refs(&cut_point);
        let messages_to_summarize = resolve_context_refs_for_compaction(
            &ctx,
            &temper_api_url,
            tenant,
            &fields,
            &context_refs,
        );
        if messages_to_summarize.is_empty() {
            ctx.log("warn", "context_compactor: no messages to summarize");
            set_compaction_skipped_result(
                &ctx,
                &temper_api_url,
                tenant,
                &fields,
                session_leaf_id,
                tree.estimate_tokens(session_leaf_id),
                "no_messages_to_summarize",
            );
            return Ok(());
        }

        let conversation_text = format_messages_for_summary(&messages_to_summarize);

        // 4. Call LLM for structured summary
        let configured_provider = fields
            .get("provider")
            .and_then(|v| v.as_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or("context_compactor requires Session.provider")?;

        let (provider, api_key) = resolve_compaction_provider(&ctx, configured_provider)?;

        let compaction_model = fields
            .get("compaction_model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                fields
                    .get("model")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            })
            .ok_or("context_compactor requires compaction_model or Session.model")?;

        let summary_result = if provider == "mock" {
            Ok(build_mock_summary(&conversation_text))
        } else {
            call_compaction_llm(
                &ctx,
                &provider,
                &api_key,
                compaction_model,
                &conversation_text,
            )
        };
        let summary = match summary_result {
            Ok(summary) => summary,
            Err(err) => {
                if let Some(reason) = compaction_auth_expired_reason(&err) {
                    set_success_result(
                        "CompactionAuthExpired",
                        &json!({
                            "provider_auth_error": reason,
                        }),
                    );
                    return Ok(());
                }
                return Err(err);
            }
        };

        ctx.log(
            "info",
            &format!(
                "context_compactor: generated summary ({} chars)",
                summary.len()
            ),
        );

        // 5. Append compaction entry to session tree
        let summary_tokens = estimate_summary_tokens(&summary);

        let entity_backed_session = is_session_entries_ref(session_file_id);
        let (compaction_id, _line) = if !entity_backed_session && !workspace_id.is_empty() {
            match create_content_file_ref(
                &ctx,
                &temper_api_url,
                tenant,
                workspace_id,
                &format!("compaction-{}.txt", tree.len()),
                &summary,
            ) {
                Ok(summary_ref) => tree.append_compaction_file(
                    session_leaf_id,
                    &summary_ref.file_id,
                    Some(&summary_ref.file_version_id),
                    &cut_point,
                    summary_tokens,
                ),
                Err(_) => tree.append_compaction(session_leaf_id, &summary, &cut_point),
            }
        } else {
            tree.append_compaction(session_leaf_id, &summary, &cut_point)
        };

        // 6. Write updated session tree back to TemperFS
        let updated_jsonl = tree.to_jsonl();
        write_session_to_temperfs(
            &ctx,
            &temper_api_url,
            tenant,
            &fields,
            session_file_id,
            &updated_jsonl,
        )?;

        // 7. Return CompactionComplete with new leaf pointing after compaction
        let new_token_estimate = tree.estimate_tokens(&compaction_id);
        set_success_result(
            "CompactionComplete",
            &json!({
                "session_leaf_id": compaction_id,
                "context_tokens": new_token_estimate,
                "compaction_skipped_leaf_id": "",
                "compaction_skipped_reason": "",
            }),
        );

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

fn set_compaction_skipped_result(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_leaf_id: &str,
    context_tokens: usize,
    reason: &str,
) {
    persist_compaction_skip_marker(ctx, temper_api_url, tenant, fields, session_leaf_id, reason);
    set_compaction_skipped_success_result(fields, session_leaf_id, context_tokens, reason);
}

fn set_compaction_skipped_success_result(
    fields: &Value,
    session_leaf_id: &str,
    context_tokens: usize,
    reason: &str,
) {
    set_success_result(
        "CompactionComplete",
        &json!({
            "session_leaf_id": session_leaf_id,
            "context_tokens": context_tokens,
            "system_prompt_hash": fields
                .get("system_prompt_hash")
                .and_then(Value::as_str)
                .unwrap_or(""),
            "system_prompt_file_id": fields
                .get("system_prompt_file_id")
                .and_then(Value::as_str)
                .unwrap_or(""),
            "compaction_skipped_leaf_id": session_leaf_id,
            "compaction_skipped_reason": reason,
        }),
    );
}

fn should_skip_repeated_compaction(fields: &Value, session_leaf_id: &str) -> bool {
    let compaction_count = read_usize_field(fields, "compaction_count").unwrap_or(0);
    if compaction_count >= 3 {
        return true;
    }

    let previous_reason = fields
        .get("compaction_skipped_reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if previous_reason.is_empty() {
        return false;
    }

    let previous_leaf = fields
        .get("compaction_skipped_leaf_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    !previous_leaf.is_empty() && previous_leaf != session_leaf_id
}

fn read_usize_field(fields: &Value, field_name: &str) -> Option<usize> {
    fields.get(field_name).and_then(|value| match value {
        Value::Number(number) => number
            .as_u64()
            .and_then(|value| usize::try_from(value).ok()),
        Value::String(value) => value.parse::<usize>().ok(),
        _ => None,
    })
}

fn persist_compaction_skip_marker(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    session_leaf_id: &str,
    reason: &str,
) {
    let session_id = fields
        .get("Id")
        .or_else(|| fields.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(&ctx.entity_id);
    let url = format!("{temper_api_url}/tdata/Sessions('{session_id}')");
    let headers = runtime_headers(
        ctx,
        tenant,
        fields,
        Some("application/json"),
        Some("application/json"),
    );
    let body = json!({
        "compaction_skipped_reason": reason,
        "compaction_skipped_leaf_id": session_leaf_id,
    });
    match ctx.http_call("PATCH", &url, &headers, &body.to_string()) {
        Ok(resp) if (200..300).contains(&resp.status) => {
            ctx.log(
                "info",
                &format!(
                    "context_compactor: persisted compaction skip marker for leaf {session_leaf_id}"
                ),
            );
        }
        Ok(resp) => {
            ctx.log(
                "warn",
                &format!(
                    "context_compactor: skip marker PATCH failed (HTTP {}): {}",
                    resp.status,
                    &resp.body[..resp.body.len().min(200)]
                ),
            );
        }
        Err(err) => {
            ctx.log(
                "warn",
                &format!("context_compactor: skip marker PATCH failed: {err}"),
            );
        }
    }
}

fn resolve_context_refs_for_compaction(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    fields: &Value,
    refs: &[session_tree_lib::ContextRef],
) -> Vec<Value> {
    let mut unique_file_version_ids = Vec::new();
    let mut unique_file_ids = Vec::new();
    let mut seen = BTreeSet::new();
    for ctx_ref in refs {
        if let Some(file_version_id) = &ctx_ref.content_file_version_id
            && seen.insert(format!("version:{file_version_id}"))
        {
            unique_file_version_ids.push(file_version_id.clone());
        }
        if let Some(file_id) = &ctx_ref.content_file_id
            && seen.insert(format!("file:{file_id}"))
        {
            unique_file_ids.push(file_id.clone());
        }
    }

    let version_batch_results = if unique_file_version_ids.len() > 1 {
        match read_text_file_versions_batch(
            ctx,
            temper_api_url,
            tenant,
            fields,
            &unique_file_version_ids,
        ) {
            Ok(results) => results,
            Err(err) => {
                ctx.log(
                    "warn",
                    &format!(
                        "context_compactor: batch file version read unavailable, falling back: {err}"
                    ),
                );
                std::collections::BTreeMap::new()
            }
        }
    } else {
        std::collections::BTreeMap::new()
    };

    let file_batch_results = if unique_file_ids.len() > 1 {
        match read_text_files_batch(ctx, temper_api_url, tenant, fields, &unique_file_ids) {
            Ok(results) => results,
            Err(err) => {
                ctx.log(
                    "warn",
                    &format!("context_compactor: batch file read unavailable, falling back: {err}"),
                );
                std::collections::BTreeMap::new()
            }
        }
    } else {
        std::collections::BTreeMap::new()
    };

    render_context_refs_for_compaction(refs, |ctx_ref| {
        if let Some(file_version_id) = &ctx_ref.content_file_version_id {
            if let Some(item) = version_batch_results.get(file_version_id) {
                return Ok(if item.found {
                    item.text.clone()
                } else {
                    String::new()
                });
            }
            match read_content_file_version(ctx, temper_api_url, tenant, fields, file_version_id) {
                Ok(raw) if !raw.is_empty() => return Ok(raw),
                Ok(_) => {}
                Err(err) => ctx.log(
                    "warn",
                    &format!(
                        "context_compactor: immutable version read unavailable for {file_version_id}, falling back to file head: {err}"
                    ),
                ),
            }
        }

        if let Some(file_id) = &ctx_ref.content_file_id {
            if let Some(item) = file_batch_results.get(file_id) {
                return Ok(if item.found {
                    item.text.clone()
                } else {
                    String::new()
                });
            }
            return read_content_file(ctx, temper_api_url, tenant, fields, file_id);
        }

        Ok(String::new())
    })
}

fn render_context_refs_for_compaction(
    refs: &[session_tree_lib::ContextRef],
    mut read_file: impl FnMut(&session_tree_lib::ContextRef) -> Result<String, String>,
) -> Vec<Value> {
    let mut messages = Vec::new();

    for ctx_ref in refs {
        match ctx_ref.entry_type {
            session_tree_lib::EntryType::Compaction => {
                let summary = if ctx_ref.content_file_id.is_some()
                    || ctx_ref.content_file_version_id.is_some()
                {
                    read_file(ctx_ref)
                        .unwrap_or_else(|_| ctx_ref.inline_summary.clone().unwrap_or_default())
                } else {
                    ctx_ref.inline_summary.clone().unwrap_or_default()
                };
                if !summary.is_empty() {
                    messages.push(json!({
                        "role": "user",
                        "content": format!("[Previous conversation summary]\n{summary}")
                    }));
                }
            }
            session_tree_lib::EntryType::Message | session_tree_lib::EntryType::Steering => {
                if ctx_ref.content_file_id.is_some() || ctx_ref.content_file_version_id.is_some() {
                    if let Ok(raw) = read_file(ctx_ref) {
                        if !raw.is_empty() {
                            let content: Value = serde_json::from_str(&raw).unwrap_or(json!(raw));
                            messages.push(json!({
                                "role": ctx_ref.role,
                                "content": content,
                            }));
                            continue;
                        }
                    }
                }
                if let Some(ref inline) = ctx_ref.inline_content {
                    messages.push(json!({
                        "role": ctx_ref.role,
                        "content": inline.clone(),
                    }));
                }
            }
            session_tree_lib::EntryType::Header => {}
        }
    }

    messages
}

fn estimate_summary_tokens(summary: &str) -> usize {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        0
    } else {
        usize::max(1, trimmed.len().div_ceil(4))
    }
}

fn build_mock_summary(conversation_text: &str) -> String {
    let truncated: String = conversation_text.chars().take(600).collect();
    format!(
        "## Active Goal\nContinue the active task.\n\n## Episodes\n\n### Episode: Prior conversation\n- **Goal:** Earlier conversation context\n- **Worked:** Conversation compacted using mock path\n- **Failed:** None recorded\n- **Discoveries:** No real model configured for compaction\n- **Artifacts:** None\n\n## Current State\n- **Where we are:** Resuming after compaction\n- **Next:** Continue the active task\n- **Open questions:** None\n\n---\nRaw context tail:\n{}",
        truncated
    )
}

/// Format messages into a text block for the compaction LLM prompt.
fn format_messages_for_summary(messages: &[Value]) -> String {
    let mut text = String::new();
    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let content = msg.get("content").cloned().unwrap_or(json!(""));
        let content_str = match content {
            Value::String(s) => s,
            Value::Array(arr) => arr
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                        block.get("text").and_then(|v| v.as_str()).map(String::from)
                    } else if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        Some(format!(
                            "[tool_use: {}]",
                            block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                        ))
                    } else if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        let content = block
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("...");
                        let truncated = if content.len() > 200 {
                            &content[..200]
                        } else {
                            content
                        };
                        Some(format!("[tool_result: {}]", truncated))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => serde_json::to_string(&content).unwrap_or_default(),
        };
        text.push_str(&format!("## {role}\n{content_str}\n\n"));
    }
    text
}

/// Check if a value is an unresolved secret template like `{secret:key_name}`.
fn is_unresolved_secret_template(value: &str) -> bool {
    value.contains("{secret:")
}

fn compaction_auth_expired_error(body: &str) -> String {
    format!(
        "{COMPACTION_AUTH_EXPIRED_PREFIX} {}",
        body.chars().take(300).collect::<String>()
    )
}

fn compaction_auth_expired_reason(error: &str) -> Option<&str> {
    error
        .strip_prefix(COMPACTION_AUTH_EXPIRED_PREFIX)
        .map(str::trim)
}

fn normalize_compaction_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "open_router" => "openrouter".to_string(),
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "hf" | "hugging_face" | "hugging-face" => "huggingface".to_string(),
        "fireworks_ai" | "fireworks-ai" => "fireworks".to_string(),
        "sakana" | "sakana-fugu" | "fugu" => "sakana_fugu".to_string(),
        "ollama" | "local" | "local-openai" => "local_openai".to_string(),
        "openai-compatible" | "openai_compat" | "openai-compat" | "custom_openai" => {
            "openai_compatible".to_string()
        }
        other => other.to_string(),
    }
}

fn is_openai_compatible_compaction_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openrouter"
            | "huggingface"
            | "fireworks"
            | "sakana_fugu"
            | "local_openai"
            | "openai_compatible"
    )
}

fn default_compaction_provider_api_url(provider: &str) -> &'static str {
    match provider {
        "openai" => "https://api.openai.com/v1/responses",
        "openai_codex" => "https://chatgpt.com/backend-api/codex/responses",
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
        "huggingface" => "https://router.huggingface.co/v1/chat/completions",
        "fireworks" => "https://api.fireworks.ai/inference/v1/chat/completions",
        "local_openai" => "http://127.0.0.1:11434/v1/chat/completions",
        "anthropic" => "https://api.anthropic.com/v1/messages",
        _ => "",
    }
}

fn configured_compaction_provider_api_url(ctx: &Context, provider: &str) -> Result<String, String> {
    let key = match provider {
        "openai" => "openai_api_url",
        "openai_codex" => "openai_codex_api_url",
        "openrouter" => "openrouter_api_url",
        "huggingface" => "huggingface_api_url",
        "fireworks" => "fireworks_api_url",
        "sakana_fugu" => "sakana_fugu_api_url",
        "openai_compatible" => "openai_compatible_api_url",
        "local_openai" => "local_openai_api_url",
        _ => "anthropic_api_url",
    };
    let api_url = ctx
        .config
        .get(key)
        .filter(|value| !value.trim().is_empty() && !is_unresolved_secret_template(value))
        .cloned()
        .unwrap_or_else(|| default_compaction_provider_api_url(provider).to_string());
    if api_url.trim().is_empty() {
        return Err(format!("provider={provider} requires {key}"));
    }
    Ok(api_url)
}

fn resolve_compaction_provider(
    ctx: &Context,
    configured_provider: &str,
) -> Result<(String, String), String> {
    let configured_provider = normalize_compaction_provider(configured_provider);
    let provider_keys: &[(&str, &[&str])] = &[
        ("anthropic", &["anthropic_api_key", "api_key"]),
        ("openai", &["openai_api_key"]),
        (
            "openai_codex",
            &["openai_codex_access_token", "openai_codex_token"],
        ),
        ("openrouter", &["openrouter_api_key"]),
        ("huggingface", &["huggingface_api_key", "hf_token", "api_key"]),
        ("fireworks", &["fireworks_api_key", "api_key"]),
        ("sakana_fugu", &["sakana_fugu_api_key", "api_key"]),
        ("openai_compatible", &["openai_compatible_api_key", "api_key"]),
        ("local_openai", &[]),
        ("mock", &[]),
    ];

    for &(prov, keys) in provider_keys {
        if prov != configured_provider {
            continue;
        }
        if matches!(prov, "mock" | "local_openai") {
            return Ok((prov.to_string(), String::new()));
        }
        for &key_name in keys {
            if let Some(val) = ctx.config.get(key_name) {
                if !val.trim().is_empty() && !is_unresolved_secret_template(val) {
                    return Ok((prov.to_string(), val.clone()));
                }
            }
        }
        if prov == "openai_compatible" {
            return Ok((prov.to_string(), String::new()));
        }
        return Err(format!(
            "missing API key for provider={configured_provider}"
        ));
    }

    Err(format!(
        "unsupported compaction provider: {configured_provider}"
    ))
}

const COMPACTION_SYSTEM_PROMPT: &str = "You are a conversation compactor. Extract distinct episodes from this conversation — each a coherent task or sub-task the agent attempted. Be concise but preserve the trajectory: what was tried, what worked, what failed, and why.\n\nOutput the summary in this exact format:\n\n## Active Goal\n<the current overarching objective>\n\n## Episodes\n\n### Episode: <short title>\n- **Goal:** <what was attempted>\n- **Worked:** <actions that succeeded and why>\n- **Failed:** <approaches tried and abandoned, what went wrong>\n- **Discoveries:** <facts learned, decisions made>\n- **Artifacts:** <files changed, entities created, useful outputs>\n\n(Repeat chronologically for each distinct episode)\n\n## Current State\n- **Where we are:** <what just completed or is in progress>\n- **Next:** <immediate next steps>\n- **Open questions:** <unresolved issues>\n\nIMPORTANT: Preserve the trajectory. A future model reading this needs to know which approaches were already tried and failed, not just what worked. This prevents repeating failed approaches.";

fn compaction_user_prompt(conversation_text: &str) -> String {
    format!("Summarize this conversation:\n\n{conversation_text}")
}

/// Build the request body for the compaction LLM call.
///
/// The OpenAI Responses API (especially the Codex backend at
/// `chatgpt.com/backend-api/codex/responses`) requires `input` to be a list of
/// input items, not a string — sending a string yields HTTP 400 "Input must be
/// a list".
fn build_compaction_request_body(
    provider: &str,
    model: &str,
    system_prompt: &str,
    conversation_text: &str,
) -> Value {
    let user_text = compaction_user_prompt(conversation_text);
    match provider {
        "openai" => json!({
            "model": model,
            "instructions": system_prompt,
            "input": [{
                "role": "user",
                "content": user_text,
            }],
            "store": false,
        }),
        "openai_codex" => json!({
            "model": model,
            "instructions": system_prompt,
            "input": [{
                "role": "user",
                "content": user_text,
            }],
            "stream": true,
            "store": false,
        }),
        provider if is_openai_compatible_compaction_provider(provider) => {
            build_chat_completion_body(
                model,
                system_prompt,
                &[json!({ "role": "user", "content": user_text })],
                &[],
                2048,
                1.0,
                false,
                false,
                "",
            )
            .unwrap_or_else(|_| {
                json!({
                    "model": model,
                    "max_tokens": 2048,
                    "messages": [
                        { "role": "system", "content": system_prompt },
                        { "role": "user", "content": compaction_user_prompt(conversation_text) },
                    ],
                })
            })
        }
        _ => json!({
            "model": model,
            "max_tokens": 2048,
            "system": system_prompt,
            "messages": [{
                "role": "user",
                "content": user_text,
            }],
        }),
    }
}

fn anthropic_compaction_headers(api_key: &str) -> Vec<(String, String)> {
    let is_oauth = api_key.contains("sk-ant-oat");
    if is_oauth {
        vec![
            ("authorization".to_string(), format!("Bearer {api_key}")),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    } else {
        vec![
            ("x-api-key".to_string(), api_key.to_string()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    }
}

/// Extract summary text from a compaction LLM response body.
///
/// `openai_codex` returns SSE (`text/event-stream`) because the Codex backend
/// requires `accept: text/event-stream`; everything else returns plain JSON.
fn parse_compaction_response_text(provider: &str, body: &str) -> String {
    const FALLBACK: &str = "Summary unavailable";
    match provider {
        "openai_codex" => parse_openai_responses_text(&collect_codex_sse_output(body))
            .unwrap_or_else(|| FALLBACK.to_string()),
        "openai" => {
            let parsed: Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(_) => return FALLBACK.to_string(),
            };
            parse_openai_responses_text(&parsed).unwrap_or_else(|| FALLBACK.to_string())
        }
        provider if is_openai_compatible_compaction_provider(provider) => {
            parse_chat_completion_response_text(body).unwrap_or_else(|_| FALLBACK.to_string())
        }
        _ => {
            let parsed: Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(_) => return FALLBACK.to_string(),
            };
            parsed
                .get("content")
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                })
                .and_then(|b| b.get("text").and_then(|v| v.as_str()))
                .unwrap_or(FALLBACK)
                .to_string()
        }
    }
}

fn parse_openai_responses_text(parsed: &Value) -> Option<String> {
    let output = parsed.get("output")?.as_array()?;
    for item in output {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
            let mut combined = String::new();
            for part in content {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        combined.push_str(text);
                    }
                }
            }
            if !combined.is_empty() {
                return Some(combined);
            }
        }
    }
    None
}

/// Collect output items and streamed text from a Codex SSE response into a
/// synthetic Responses-API JSON value that `parse_openai_responses_text` can
/// consume.
fn collect_codex_sse_output(body: &str) -> Value {
    let mut output_items: Vec<Value> = Vec::new();
    let mut streamed_text = String::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line == "[DONE]" {
            continue;
        }
        let json_str = line.strip_prefix("data: ").unwrap_or(line);
        let event: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match event_type {
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    output_items.push(item.clone());
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                    streamed_text.push_str(delta);
                } else if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                    streamed_text.push_str(text);
                }
            }
            "response.output_text.done" => {
                if streamed_text.is_empty() {
                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                        streamed_text.push_str(text);
                    }
                }
            }
            "response.completed" => {
                if let Some(out) = event
                    .get("response")
                    .and_then(|r| r.get("output"))
                    .and_then(|v| v.as_array())
                {
                    if !out.is_empty() {
                        output_items = out.clone();
                    }
                }
            }
            _ => {}
        }
    }

    if output_items.is_empty() {
        let trimmed = streamed_text.trim();
        if !trimmed.is_empty() {
            output_items.push(json!({
                "type": "message",
                "content": [{ "type": "output_text", "text": trimmed }],
            }));
        }
    }

    json!({ "output": output_items })
}

/// Call the LLM with a compaction-specific system prompt.
fn call_compaction_llm(
    ctx: &Context,
    provider: &str,
    api_key: &str,
    model: &str,
    conversation_text: &str,
) -> Result<String, String> {
    let body =
        build_compaction_request_body(provider, model, COMPACTION_SYSTEM_PROMPT, conversation_text);
    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("JSON serialize error: {e}"))?;
    let url = configured_compaction_provider_api_url(ctx, provider)?;

    let headers = match provider {
        "openai" | "openai_codex" => {
            let codex_account_id = if provider == "openai_codex" {
                ctx.config
                    .get("openai_codex_account_id")
                    .filter(|value| {
                        !value.trim().is_empty() && !is_unresolved_secret_template(value)
                    })
                    .cloned()
                    .or_else(|| extract_chatgpt_account_id_from_jwt(api_key))
            } else {
                None
            };
            build_openai_headers(provider, api_key, codex_account_id.as_deref())
        }
        provider if is_openai_compatible_compaction_provider(provider) => {
            let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
            if !api_key.trim().is_empty() {
                headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
            }
            if provider == "openai_compatible" {
                let headers_json = ctx
                    .config
                    .get("openai_compatible_headers_json")
                    .filter(|value| !is_unresolved_secret_template(value))
                    .map(String::as_str)
                    .unwrap_or("");
                headers.extend(parse_headers_json(headers_json)?);
            }
            headers
        }
        _ => anthropic_compaction_headers(api_key),
    };

    ctx.log(
        "info",
        &format!("context_compactor: calling {provider} at {url} with model={model}"),
    );
    let resp = ctx.http_call("POST", &url, &headers, &body_str)?;
    if resp.status != 200 {
        if provider == "openai_codex"
            && is_openai_codex_token_expired_error(resp.status as u16, &resp.body)
        {
            return Err(compaction_auth_expired_error(&resp.body));
        }
        return Err(format!(
            "Compaction LLM call failed (HTTP {}): {}",
            resp.status,
            &resp.body[..resp.body.len().min(500)]
        ));
    }

    Ok(parse_compaction_response_text(provider, &resp.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compaction_response_handles_openai_responses_json() {
        let body = json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "the summary" }],
            }]
        })
        .to_string();
        assert_eq!(
            parse_compaction_response_text("openai", &body),
            "the summary"
        );
    }

    #[test]
    fn parse_compaction_response_handles_codex_sse_stream() {
        // Codex backend returns text/event-stream — the previous code blindly
        // ran serde_json on the whole body and produced "Summary unavailable".
        let body = "\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello \"}\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\
data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\
data: [DONE]\n";
        assert_eq!(
            parse_compaction_response_text("openai_codex", body),
            "hello world"
        );
    }

    #[test]
    fn parse_compaction_response_handles_anthropic_messages_json() {
        let body = json!({
            "content": [{ "type": "text", "text": "anthropic summary" }]
        })
        .to_string();
        assert_eq!(
            parse_compaction_response_text("anthropic", &body),
            "anthropic summary"
        );
    }

    #[test]
    fn parse_compaction_response_handles_openrouter_chat_completions() {
        let body = json!({
            "choices": [{ "message": { "content": "router summary" } }]
        })
        .to_string();
        assert_eq!(
            parse_compaction_response_text("openrouter", &body),
            "router summary"
        );
    }

    #[test]
    fn parse_compaction_response_falls_back_when_body_unparseable() {
        assert_eq!(
            parse_compaction_response_text("openai", "not json"),
            "Summary unavailable"
        );
    }
}
