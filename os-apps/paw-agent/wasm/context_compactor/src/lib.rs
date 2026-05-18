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
use session_tree_lib::SessionTree;
use std::collections::BTreeSet;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    create_content_file_ref, is_session_entries_ref, read_content_file, read_content_file_version,
    read_session_from_temperfs, read_text_file_versions_batch, read_text_files_batch,
    resolve_temper_api_url, write_session_to_temperfs,
};

const COMPACTION_AUTH_EXPIRED_PREFIX: &str = "compaction_auth_expired:";

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

        // 2. Find cut point
        let cut_point = match tree.find_cut_point(session_leaf_id, keep_recent_tokens) {
            Some(cp) => cp,
            None => {
                ctx.log(
                    "warn",
                    "context_compactor: no valid cut point found, skipping compaction",
                );
                set_success_result(
                    "CompactionComplete",
                    &json!({
                        "session_leaf_id": session_leaf_id,
                        "context_tokens": tree.estimate_tokens(session_leaf_id),
                    }),
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
            set_success_result(
                "CompactionComplete",
                &json!({
                    "session_leaf_id": session_leaf_id,
                    "context_tokens": tree.estimate_tokens(session_leaf_id),
                }),
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
        } else if compaction_should_use_local_fallback(&provider) {
            ctx.log(
                "warn",
                "context_compactor: using local fallback summary for openai_codex provider",
            );
            Ok(build_local_fallback_summary(
                &conversation_text,
                "openai_codex compaction uses local fallback",
            ))
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
                compaction_summary_or_fallback(&conversation_text, Err(err))?
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
            }),
        );

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
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

fn compaction_summary_or_fallback(
    conversation_text: &str,
    result: Result<String, String>,
) -> Result<String, String> {
    match result {
        Ok(summary) => Ok(summary),
        Err(err) if compaction_auth_expired_reason(&err).is_some() => Err(err),
        Err(err) => Ok(build_local_fallback_summary(conversation_text, &err)),
    }
}

fn compaction_should_use_local_fallback(provider: &str) -> bool {
    provider == "openai_codex"
}

fn build_local_fallback_summary(conversation_text: &str, error: &str) -> String {
    let excerpt: String = conversation_text.chars().take(1200).collect();
    let failure_class = if error.contains("HTTP call failed")
        || error.contains("host_http_call")
        || error.contains("deadline")
        || error.contains("timeout")
    {
        "provider transport timeout"
    } else {
        "provider compaction error"
    };

    format!(
        "## Active Goal\nContinue the active task.\n\n## Episodes\n\n### Episode: Prior conversation\n- **Goal:** Preserve working context after the compaction provider was unavailable.\n- **Worked:** Earlier conversation was compacted using local fallback compaction.\n- **Failed:** Automated LLM compaction hit a {failure_class}.\n- **Discoveries:** Use the raw context excerpt below when resuming.\n- **Artifacts:** None recorded by fallback compaction.\n\n## Current State\n- **Where we are:** Resuming after local fallback compaction.\n- **Next:** Continue the active task from recent context and the excerpt.\n- **Open questions:** None recorded by fallback compaction.\n\n---\nRaw context excerpt:\n{}",
        excerpt
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

fn default_compaction_provider_api_url(provider: &str) -> &'static str {
    match provider {
        "openai" => "https://api.openai.com/v1/responses",
        "openai_codex" => "https://chatgpt.com/backend-api/codex/responses",
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
        _ => "https://api.anthropic.com/v1/messages",
    }
}

fn configured_compaction_provider_api_url(ctx: &Context, provider: &str) -> String {
    let key = match provider {
        "openai" => "openai_api_url",
        "openai_codex" => "openai_codex_api_url",
        "openrouter" => "openrouter_api_url",
        _ => "anthropic_api_url",
    };
    ctx.config
        .get(key)
        .cloned()
        .unwrap_or_else(|| default_compaction_provider_api_url(provider).to_string())
}

fn resolve_compaction_provider(
    ctx: &Context,
    configured_provider: &str,
) -> Result<(String, String), String> {
    let provider_keys: &[(&str, &[&str])] = &[
        ("anthropic", &["anthropic_api_key", "api_key"]),
        ("openai", &["openai_api_key"]),
        (
            "openai_codex",
            &["openai_codex_access_token", "openai_codex_token"],
        ),
        ("openrouter", &["openrouter_api_key"]),
        ("mock", &[]),
    ];

    for &(prov, keys) in provider_keys {
        if prov != configured_provider {
            continue;
        }
        if prov == "mock" {
            return Ok((prov.to_string(), String::new()));
        }
        for &key_name in keys {
            if let Some(val) = ctx.config.get(key_name) {
                if !val.trim().is_empty() && !is_unresolved_secret_template(val) {
                    return Ok((prov.to_string(), val.clone()));
                }
            }
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
        "openrouter" => json!({
            "model": model,
            "max_tokens": 2048,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_text },
            ],
        }),
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
        "openrouter" => {
            let parsed: Value = match serde_json::from_str(body) {
                Ok(v) => v,
                Err(_) => return FALLBACK.to_string(),
            };
            parsed
                .get("choices")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or(FALLBACK)
                .to_string()
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
    let url = configured_compaction_provider_api_url(ctx, provider);

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
        "openrouter" => vec![
            ("authorization".to_string(), format!("Bearer {api_key}")),
            ("content-type".to_string(), "application/json".to_string()),
        ],
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
    use session_tree_lib::EntryType;

    #[test]
    fn render_context_refs_for_compaction_uses_loaded_file_content_and_inline_fallbacks() {
        let refs = vec![
            session_tree_lib::ContextRef {
                entry_id: "cmp-1".to_string(),
                role: "user".to_string(),
                content_file_id: Some("cmp-file".to_string()),
                content_file_version_id: Some("cmp-ver".to_string()),
                entry_type: EntryType::Compaction,
                inline_content: None,
                inline_summary: Some("inline summary".to_string()),
            },
            session_tree_lib::ContextRef {
                entry_id: "msg-1".to_string(),
                role: "assistant".to_string(),
                content_file_id: Some("msg-file".to_string()),
                content_file_version_id: Some("msg-ver".to_string()),
                entry_type: EntryType::Message,
                inline_content: None,
                inline_summary: None,
            },
            session_tree_lib::ContextRef {
                entry_id: "steer-1".to_string(),
                role: "system".to_string(),
                content_file_id: Some("steer-file".to_string()),
                content_file_version_id: Some("steer-ver".to_string()),
                entry_type: EntryType::Steering,
                inline_content: Some(json!("inline steering")),
                inline_summary: None,
            },
        ];

        let rendered =
            render_context_refs_for_compaction(&refs, |ctx_ref| match ctx_ref.entry_id.as_str() {
                "cmp-1" => Err("blob missing".to_string()),
                "msg-1" => Ok("{\"type\":\"text\",\"text\":\"hello\"}".to_string()),
                "steer-1" => Ok(String::new()),
                other => Err(format!("unexpected context ref: {other}")),
            });

        assert_eq!(rendered.len(), 3);
        assert_eq!(
            rendered[0]["content"],
            "[Previous conversation summary]\ninline summary"
        );
        assert_eq!(rendered[1]["role"], "assistant");
        assert_eq!(rendered[1]["content"]["text"], "hello");
        assert_eq!(rendered[2]["role"], "system");
        assert_eq!(rendered[2]["content"], "inline steering");
    }

    #[test]
    fn compaction_provider_defaults_keep_openai_and_codex_separate() {
        assert_eq!(
            default_compaction_provider_api_url("openai"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            default_compaction_provider_api_url("openai_codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn openai_compaction_body_sends_input_as_list_not_string() {
        // Regression: the Codex Responses backend rejects string `input` with
        // HTTP 400 "Input must be a list".
        for provider in ["openai", "openai_codex"] {
            let body = build_compaction_request_body(
                provider,
                "gpt-test",
                "system text",
                "conversation text",
            );
            assert_eq!(body["model"], "gpt-test");
            assert_eq!(body["instructions"], "system text");
            let input = body
                .get("input")
                .unwrap_or_else(|| panic!("{provider}: missing input"));
            let arr = input
                .as_array()
                .unwrap_or_else(|| panic!("{provider}: input must be a list, got {input:?}"));
            assert_eq!(arr.len(), 1, "{provider}: expected single input item");
            assert_eq!(arr[0]["role"], "user");
            assert!(
                arr[0]["content"]
                    .as_str()
                    .unwrap_or("")
                    .contains("conversation text"),
                "{provider}: user content must include conversation text"
            );
            assert_eq!(body["store"], json!(false));
        }
    }

    #[test]
    fn codex_compaction_body_requests_streaming_response() {
        // Regression: the ChatGPT Codex Responses backend pairs the
        // `text/event-stream` contract with a required `stream: true` body
        // field, otherwise it returns HTTP 400 "Stream must be set to true".
        let body = build_compaction_request_body(
            "openai_codex",
            "gpt-test",
            "system text",
            "conversation text",
        );

        assert_eq!(body["stream"], json!(true));
    }

    #[test]
    fn openrouter_compaction_body_uses_chat_messages() {
        let body =
            build_compaction_request_body("openrouter", "anthropic/claude-3-haiku", "sys", "convo");
        let messages = body["messages"].as_array().expect("messages must be array");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "sys");
        assert_eq!(messages[1]["role"], "user");
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap_or("")
                .contains("convo")
        );
    }

    #[test]
    fn anthropic_compaction_body_uses_messages_with_system_field() {
        let body = build_compaction_request_body("anthropic", "claude-test", "sys", "convo");
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["system"], "sys");
        let messages = body["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

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
    fn parse_compaction_response_codex_prefers_completed_output_when_present() {
        let body = "\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\
data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"final summary\"}]}]}}\n";
        assert_eq!(
            parse_compaction_response_text("openai_codex", body),
            "final summary"
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

    #[test]
    fn compaction_auth_expired_result_is_action_routable() {
        let body = r#"{"error":{"code":"token_expired"}}"#;
        let err = compaction_auth_expired_error(body);

        assert_eq!(compaction_auth_expired_reason(&err), Some(body));
        assert_eq!(
            compaction_auth_expired_reason("regular compaction failure"),
            None
        );
    }

    #[test]
    fn compaction_transport_failure_uses_local_fallback_summary() {
        let summary = compaction_summary_or_fallback(
            "## user\nplease continue the incident investigation",
            Err(
                "HTTP call failed: POST https://chatgpt.com/backend-api/codex/responses"
                    .to_string(),
            ),
        )
        .expect("transport failures should fall back locally");

        assert!(summary.contains("local fallback compaction"));
        assert!(summary.contains("please continue the incident investigation"));
        assert!(
            !summary.contains("chatgpt.com/backend-api/codex/responses"),
            "fallback summary must not preserve raw provider URLs"
        );
    }

    #[test]
    fn compaction_auth_expired_still_routes_to_auth_recovery() {
        let err = compaction_auth_expired_error(r#"{"error":{"code":"token_expired"}}"#);

        assert_eq!(
            compaction_summary_or_fallback("conversation", Err(err.clone())),
            Err(err)
        );
    }

    #[test]
    fn openai_codex_compaction_uses_local_fallback_strategy() {
        assert!(compaction_should_use_local_fallback("openai_codex"));
        assert!(!compaction_should_use_local_fallback("openai"));
        assert!(!compaction_should_use_local_fallback("anthropic"));
    }
}
