//! Provider Caller — staged Session-turn WASM for LLM provider I/O.
//!
//! Owns the `CallingProvider` phase:
//! - resolve provider/model/api-key inputs
//! - translate the prepared artifact into provider wire formats
//! - perform outbound LLM HTTP and retries
//! - record usage/observability metadata
//! - write the provider-response artifact
//! - route to `ProviderResponseReady`
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use openai_chat_wire::{
    ChatCompletionStreamAccumulator, ChatStreamDelta, ChatStreamParseFailure,
    build_chat_completion_body, convert_messages_to_chat, event_token_signals,
    merge_token_signals, parse_headers_json, synthetic_tool_call_id,
};
#[cfg(test)]
use openai_codex_wire::base64_url_no_pad;
use openai_codex_wire::{
    build_openai_headers, extract_chatgpt_account_id_from_jwt, is_openai_codex_token_expired_error,
    select_openai_responses_url,
};
use session_turn_artifacts::{
    PreparedContextArtifact, ProviderResponseArtifact, build_gen_ai_input_messages,
    build_gen_ai_output_messages, build_gen_ai_system_instructions,
    build_provider_response_ready_params_with_inline, parse_prepared_context_artifact,
};
use std::collections::BTreeMap;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    read_content_file, resolve_temper_api_url, runtime_headers, runtime_headers_as,
    send_typing_indicator, timestamp_millis_string,
};

const DEFAULT_PROVIDER_CALLER_BUDGET_MS: i64 = 600_000;
const PROVIDER_AUTH_EXPIRED_PREFIX: &str = "provider_auth_expired:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderProgressBoundary {
    Start,
    End,
}

fn run_with_provider_progress<T>(
    mut emit_progress: impl FnMut(ProviderProgressBoundary),
    call_provider: impl FnOnce() -> T,
) -> T {
    emit_progress(ProviderProgressBoundary::Start);
    let result = call_provider();
    emit_progress(ProviderProgressBoundary::End);
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_provider_caller() {
        set_error_result(&err);
    }
    0
}

/// Parsed LLM response.
struct LlmResponse {
    content: Value,
    stop_reason: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    request_bytes: usize,
    response_bytes: usize,
    /// Token-level RL signals the serving stack returned, when it returned any.
    token_signals: Option<Value>,
}

fn normalize_provider(provider: &str) -> String {
    let norm = provider.trim().to_ascii_lowercase();
    match norm.as_str() {
        "open_router" => "openrouter".to_string(),
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "hf" | "hugging_face" | "hugging-face" => "huggingface".to_string(),
        "fireworks_ai" | "fireworks-ai" => "fireworks".to_string(),
        "sakana" | "sakana-fugu" | "fugu" => "sakana_fugu".to_string(),
        "ollama" | "local" | "local-openai" => "local_openai".to_string(),
        "openai-compatible" | "openai_compat" | "openai-compat" | "custom_openai" => {
            "openai_compatible".to_string()
        }
        _ => norm,
    }
}

fn is_unresolved_secret_template(value: &str) -> bool {
    value.contains("{secret:")
}

fn provider_auth_expired_error(body: &str) -> String {
    format!(
        "{PROVIDER_AUTH_EXPIRED_PREFIX} {}",
        body.chars().take(300).collect::<String>()
    )
}

fn provider_auth_expired_reason(error: &str) -> Option<&str> {
    error
        .strip_prefix(PROVIDER_AUTH_EXPIRED_PREFIX)
        .map(str::trim)
}

fn first_non_empty(values: &[Option<String>]) -> String {
    for v in values.iter().flatten() {
        if !v.trim().is_empty() && !is_unresolved_secret_template(v) {
            return v.trim().to_string();
        }
    }
    String::new()
}

fn resolve_provider_api_key(ctx: &Context, provider: &str) -> Result<String, String> {
    let key = match provider {
        "anthropic" => first_non_empty(&[
            ctx.config.get("anthropic_api_key").cloned(),
            ctx.config.get("anthropic_api_token").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "openai" => first_non_empty(&[
            ctx.config.get("openai_api_key").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "openai_codex" => first_non_empty(&[
            ctx.config.get("openai_codex_access_token").cloned(),
            ctx.config.get("openai_codex_token").cloned(),
        ]),
        "openrouter" => first_non_empty(&[
            ctx.config.get("openrouter_api_key").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "huggingface" => first_non_empty(&[
            ctx.config.get("huggingface_api_key").cloned(),
            ctx.config.get("hf_token").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "fireworks" => first_non_empty(&[
            ctx.config.get("fireworks_api_key").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "sakana_fugu" => first_non_empty(&[
            ctx.config.get("sakana_fugu_api_key").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "openai_compatible" => first_non_empty(&[
            ctx.config.get("openai_compatible_api_key").cloned(),
            ctx.config.get("api_key").cloned(),
        ]),
        "local_openai" => String::new(),
        other => return Err(format!("unsupported LLM provider: {other}")),
    };
    Ok(key)
}

fn default_openai_compatible_api_url(provider: &str) -> &'static str {
    match provider {
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
        "huggingface" => "https://router.huggingface.co/v1/chat/completions",
        "fireworks" => "https://api.fireworks.ai/inference/v1/chat/completions",
        "local_openai" => "http://127.0.0.1:11434/v1/chat/completions",
        _ => "",
    }
}

fn configured_openai_compatible_api_url(ctx: &Context, provider: &str) -> Result<String, String> {
    let config_key = match provider {
        "openrouter" => "openrouter_api_url",
        "huggingface" => "huggingface_api_url",
        "fireworks" => "fireworks_api_url",
        "sakana_fugu" => "sakana_fugu_api_url",
        "openai_compatible" => "openai_compatible_api_url",
        "local_openai" => "local_openai_api_url",
        other => return Err(format!("provider={other} is not OpenAI-compatible")),
    };
    let configured = ctx
        .config
        .get(config_key)
        .filter(|value| !value.trim().is_empty() && !is_unresolved_secret_template(value))
        .cloned();
    let api_url =
        configured.unwrap_or_else(|| default_openai_compatible_api_url(provider).to_string());
    if api_url.trim().is_empty() {
        return Err(format!("provider={provider} requires {config_key}"));
    }
    Ok(api_url)
}

fn configured_openai_compatible_headers(
    ctx: &Context,
    provider: &str,
) -> Result<Vec<(String, String)>, String> {
    if provider != "openai_compatible" {
        return Ok(Vec::new());
    }
    let headers_json = ctx
        .config
        .get("openai_compatible_headers_json")
        .filter(|value| !is_unresolved_secret_template(value))
        .map(String::as_str)
        .unwrap_or("");
    parse_headers_json(headers_json)
}

fn provider_allows_empty_api_key(provider: &str) -> bool {
    matches!(provider, "mock" | "local_openai" | "openai_compatible")
}

fn call_mock(
    ctx: &Context,
    messages: &[Value],
    assembled_system_prompt: &str,
    _tools: &[Value],
) -> Result<LlmResponse, String> {
    ctx.log("info", "session_turn: using deterministic mock provider");
    if mock_plan_requests_hang(messages) {
        simulate_mock_hang(ctx)?;
        return Err("mock hang scenario finished without heartbeat".to_string());
    }

    let assistant_turns = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .count();

    if let Some(step) =
        extract_mock_plan(messages).and_then(|steps| steps.get(assistant_turns).cloned())
    {
        return build_mock_step_response(messages, assembled_system_prompt, assistant_turns, &step);
    }

    let latest_user = latest_user_text(messages);
    let text = resolve_mock_template(
        latest_user
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("mock provider completed"),
        assembled_system_prompt,
        latest_user.as_deref().unwrap_or(""),
    );
    Ok(mock_text_response(messages, text))
}

// Dead mock-analysis helpers removed (extract_mock_signal_summary, build_mock_analysis,
// collect_existing_dedupe_keys, lookup_string, lookup_u64, lookup_f64, normalize_key,
// humanize_issue_focus). Recoverable from git history if needed.

fn detect_anthropic_oauth_mode(_api_key: &str, auth_mode: &str) -> bool {
    match auth_mode.trim().to_ascii_lowercase().as_str() {
        "oauth" | "token" | "bearer" => true,
        "api_key" => false,
        // Auto-detect: Anthropic accepts all token formats via x-api-key header.
        // Only use Bearer if explicitly configured. Default to x-api-key.
        _ => false,
    }
}

/// Call Anthropic Messages API.
/// Hang-hint threshold for a single LLM HTTP attempt. 60 s is chosen so
/// the hint fires well before the `provider_caller` integration's 600 s
/// WASM-host timeout (see `os-apps/paw-agent/specs/session.ioa.toml`),
/// which is when the WASM host kills the module with no further logs.
const LLM_HANG_HINT_THRESHOLD_MS: i64 = 60_000;

/// True iff a single LLM HTTP attempt has already exceeded the
/// hang-hint threshold. Matches ADR-0037 Fix B.
fn should_emit_hang_hint(attempt_elapsed_ms: i64) -> bool {
    attempt_elapsed_ms >= LLM_HANG_HINT_THRESHOLD_MS
}

/// Log line emitted immediately before an LLM HTTP attempt.
fn format_llm_attempt_start_log(
    provider: &str,
    model: &str,
    attempt: u32,
    total: u32,
    total_elapsed_ms: i64,
) -> String {
    format!(
        "session_turn: {provider} attempt {attempt}/{total} start total_elapsed_ms={total_elapsed_ms} model={model}"
    )
}

/// Log line emitted after an LLM HTTP attempt returns.
fn format_llm_attempt_end_log(
    provider: &str,
    attempt: u32,
    attempt_elapsed_ms: i64,
    http_status: u16,
    body_len: usize,
) -> String {
    format!(
        "session_turn: {provider} attempt {attempt} end elapsed_ms={attempt_elapsed_ms} http_status={http_status} body_len={body_len}"
    )
}

/// Log line emitted when the LLM call completes (success or failure).
fn format_llm_complete_log(
    provider: &str,
    model: &str,
    attempts: u32,
    total_elapsed_ms: i64,
    outcome: &str,
) -> String {
    format!(
        "session_turn: {provider} complete attempts={attempts} total_elapsed_ms={total_elapsed_ms} model={model} outcome={outcome}"
    )
}

/// Warn-level hang-hint line fired when a single attempt crosses
/// `LLM_HANG_HINT_THRESHOLD_MS`. Surfaces in DD so operators see the
/// slow call before the 600 s WASM timeout kills the module silently.
fn format_llm_hang_hint(provider: &str, attempt: u32, attempt_elapsed_ms: i64) -> String {
    format!(
        "session_turn: HANG HINT {provider} attempt {attempt} took {attempt_elapsed_ms} ms (>= {} ms). \
         Upstream provider may be hung; WASM integration timeout_secs will kill the module.",
        LLM_HANG_HINT_THRESHOLD_MS
    )
}

fn format_openai_codex_host_http_failure_log(attempt: u32, total: u32, err: &str) -> String {
    format!(
        "session_turn: OpenAI Codex host HTTP call failed before a provider HTTP response was returned \
         attempt={attempt}/{total} host_http_timeout_or_transport_error=true error={err}"
    )
}

fn format_openai_codex_exhausted_error(attempts: u32, last_err: &str) -> String {
    format!(
        "OpenAI Codex host HTTP call failed after {attempts} attempts before a provider HTTP response was returned \
         (host HTTP timeout or transport error): {last_err}"
    )
}

const LLM_STREAM_PROGRESS_INTERVAL_MS: i64 = 15_000;
const LLM_STREAM_PROGRESS_BYTES: usize = 16 * 1024;
#[cfg(target_arch = "wasm32")]
const LLM_STREAM_READ_BUFFER_BYTES: usize = 16 * 1024;
#[cfg(target_arch = "wasm32")]
const LLM_STREAM_REQUEST_CHUNK_BYTES: usize = 16 * 1024;
const LLM_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LlmStreamDelta {
    delta_text: String,
    accumulated_text_chars: usize,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_arguments_delta: Option<String>,
}

impl LlmStreamDelta {
    fn text(delta_text: &str, accumulated_text_chars: usize) -> Self {
        Self {
            delta_text: delta_text.to_string(),
            accumulated_text_chars,
            tool_call_id: None,
            tool_name: None,
            tool_arguments_delta: None,
        }
    }

    fn tool(
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        tool_arguments_delta: Option<String>,
        accumulated_text_chars: usize,
    ) -> Self {
        Self {
            delta_text: String::new(),
            accumulated_text_chars,
            tool_call_id,
            tool_name,
            tool_arguments_delta,
        }
    }

    fn is_semantic(&self) -> bool {
        !self.delta_text.is_empty()
            || self.tool_call_id.is_some()
            || self.tool_name.is_some()
            || self.tool_arguments_delta.is_some()
    }
}

fn chat_deltas_to_llm(deltas: Vec<ChatStreamDelta>) -> Vec<LlmStreamDelta> {
    deltas
        .into_iter()
        .map(|delta| LlmStreamDelta {
            delta_text: delta.delta_text,
            accumulated_text_chars: delta.accumulated_text_chars,
            tool_call_id: delta.tool_call_id,
            tool_name: delta.tool_name,
            tool_arguments_delta: delta.tool_arguments_delta,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct ParsedProviderStream {
    content: Value,
    stop_reason: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    response_bytes: usize,
    semantic_deltas: Vec<LlmStreamDelta>,
    completed: bool,
    /// Token-level RL signals the serving stack streamed, when it streamed any.
    token_signals: Option<Value>,
}

impl ParsedProviderStream {
    fn into_llm_response(self, request_bytes: usize) -> LlmResponse {
        LlmResponse {
            content: self.content,
            stop_reason: self.stop_reason,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            request_bytes,
            response_bytes: self.response_bytes,
            token_signals: self.token_signals,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamParseFailure {
    message: String,
    semantic_output_seen: bool,
}

impl StreamParseFailure {
    fn new(message: impl Into<String>, semantic_output_seen: bool) -> Self {
        Self {
            message: message.into(),
            semantic_output_seen,
        }
    }
}

impl std::fmt::Display for StreamParseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn chat_stream_error(err: ChatStreamParseFailure) -> StreamParseFailure {
    StreamParseFailure::new(err.to_string(), err.semantic_output_seen)
}

fn should_retry_stream_failure(
    attempt: u32,
    max_attempts: u32,
    semantic_output_seen: bool,
) -> bool {
    attempt < max_attempts && !semantic_output_seen
}

#[derive(Default)]
struct SseDataDecoder {
    pending: String,
}

impl SseDataDecoder {
    fn push_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.push_str(&String::from_utf8_lossy(chunk));
        self.drain_complete_lines(false)
    }

    fn finish(&mut self) -> Vec<String> {
        self.drain_complete_lines(true)
    }

    fn drain_complete_lines(&mut self, include_partial: bool) -> Vec<String> {
        let mut events = Vec::new();
        loop {
            let Some(newline) = self.pending.find('\n') else {
                break;
            };
            let line = self.pending[..newline].trim_end_matches('\r').to_string();
            self.pending = self.pending[newline + 1..].to_string();
            push_sse_data_line(&line, &mut events);
        }

        if include_partial {
            let line = self.pending.trim_end_matches('\r').to_string();
            self.pending.clear();
            push_sse_data_line(&line, &mut events);
        }

        events
    }
}

fn push_sse_data_line(line: &str, events: &mut Vec<String>) {
    let Some(data) = line.strip_prefix("data:") else {
        return;
    };
    let data = data.trim_start();
    if !data.is_empty() {
        events.push(data.to_string());
    }
}

#[cfg(any(test, not(target_arch = "wasm32")))]
fn collect_sse_data_events(chunks: &[&[u8]]) -> Result<Vec<String>, String> {
    let mut decoder = SseDataDecoder::default();
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push_chunk(chunk));
    }
    events.extend(decoder.finish());
    Ok(events)
}

#[cfg(test)]
fn stream_chunks_response_bytes(chunks: &[&[u8]]) -> usize {
    chunks.iter().map(|chunk| chunk.len()).sum()
}

#[cfg(test)]
fn parse_openai_stream_chunks(
    chunks: &[&[u8]],
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let events =
        collect_sse_data_events(chunks).map_err(|err| StreamParseFailure::new(err, false))?;
    parse_openai_stream_events(&events, stream_chunks_response_bytes(chunks))
}

#[cfg(test)]
fn parse_anthropic_stream_chunks(
    chunks: &[&[u8]],
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let events =
        collect_sse_data_events(chunks).map_err(|err| StreamParseFailure::new(err, false))?;
    parse_anthropic_stream_events(&events, stream_chunks_response_bytes(chunks))
}

#[cfg(test)]
fn parse_openrouter_stream_chunks(
    chunks: &[&[u8]],
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let events =
        collect_sse_data_events(chunks).map_err(|err| StreamParseFailure::new(err, false))?;
    parse_openrouter_stream_events(&events, stream_chunks_response_bytes(chunks))
}

#[derive(Default)]
struct OpenAiStreamAccumulator {
    output_items: Vec<Value>,
    usage: Value,
    streamed_text: String,
    saw_completed: bool,
    semantic_deltas: Vec<LlmStreamDelta>,
    token_signals: Option<Value>,
}

impl OpenAiStreamAccumulator {
    fn ingest_data(&mut self, data: &str) -> Result<Vec<LlmStreamDelta>, StreamParseFailure> {
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }

        let event: Value = serde_json::from_str(data).map_err(|err| {
            StreamParseFailure::new(
                format!("parse OpenAI stream event: {err}"),
                self.semantic_output_seen(),
            )
        })?;
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let mut deltas = Vec::new();
        match event_type {
            "response.output_text.delta" => {
                let delta = event
                    .get("delta")
                    .or_else(|| event.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !delta.is_empty() {
                    self.streamed_text.push_str(delta);
                    deltas.push(LlmStreamDelta::text(
                        delta,
                        self.streamed_text.chars().count(),
                    ));
                }
            }
            "response.output_text.done" => {
                if self.streamed_text.is_empty()
                    && let Some(text) = event.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.streamed_text.push_str(text);
                    deltas.push(LlmStreamDelta::text(
                        text,
                        self.streamed_text.chars().count(),
                    ));
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        deltas.push(LlmStreamDelta::tool(
                            item.get("call_id")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            item.get("name").and_then(Value::as_str).map(str::to_string),
                            item.get("arguments")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            self.streamed_text.chars().count(),
                        ));
                    }
                    self.output_items.push(item.clone());
                }
            }
            "response.completed" => {
                self.saw_completed = true;
                if let Some(resp) = event.get("response") {
                    // One event contributes each signal once — the response is
                    // the content level, `response.usage` the accounting one.
                    // See `event_token_signals`.
                    if let Some(source) = event_token_signals(resp.get("usage"), Some(resp)) {
                        merge_token_signals(&mut self.token_signals, &source);
                    }
                    if let Some(usage) = resp.get("usage") {
                        self.usage = usage.clone();
                    }
                    if let Some(out) = resp.get("output").and_then(Value::as_array)
                        && !out.is_empty()
                    {
                        merge_openai_completed_output_items(&mut self.output_items, out);
                    }
                }
            }
            "error" => {
                return Err(StreamParseFailure::new(
                    format!("OpenAI stream error event: {event}"),
                    self.semantic_output_seen(),
                ));
            }
            _ => {}
        }
        self.semantic_deltas.extend(deltas.clone());
        Ok(deltas)
    }

    fn semantic_output_seen(&self) -> bool {
        !self.streamed_text.is_empty()
            || !self.output_items.is_empty()
            || self.semantic_deltas.iter().any(LlmStreamDelta::is_semantic)
    }

    fn finalize(self, response_bytes: usize) -> Result<ParsedProviderStream, StreamParseFailure> {
        if !self.saw_completed {
            return Err(StreamParseFailure::new(
                "OpenAI SSE stream ended before response.completed",
                self.semantic_output_seen(),
            ));
        }

        let (mut content_blocks, has_tool_calls) =
            openai_output_items_to_content_blocks(&self.output_items);
        if content_blocks.is_empty() && !self.streamed_text.trim().is_empty() {
            content_blocks.push(json!({
                "type": "text",
                "text": self.streamed_text.trim(),
            }));
        }

        Ok(ParsedProviderStream {
            content: Value::Array(content_blocks),
            stop_reason: if has_tool_calls {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            },
            input_tokens: self
                .usage
                .get("input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            output_tokens: self
                .usage
                .get("output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            response_bytes,
            semantic_deltas: self.semantic_deltas,
            completed: true,
            token_signals: self.token_signals,
        })
    }
}

fn merge_openai_completed_output_items(output_items: &mut Vec<Value>, completed_output: &[Value]) {
    if output_items.is_empty() {
        output_items.extend(completed_output.iter().cloned());
        return;
    }

    for item in completed_output {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        let duplicate = if item_type == "function_call" {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            output_items.iter().any(|existing| {
                existing.get("type").and_then(Value::as_str) == Some("function_call")
                    && existing
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        == call_id
            })
        } else {
            output_items.iter().any(|existing| existing == item)
        };

        if !duplicate {
            output_items.push(item.clone());
        }
    }
}

fn openai_output_items_to_content_blocks(output_items: &[Value]) -> (Vec<Value>, bool) {
    let mut content_blocks = Vec::<Value>::new();
    let mut has_tool_calls = false;

    for item in output_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "message" => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text")
                            && let Some(text) = part.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            content_blocks.push(json!({
                                "type": "text",
                                "text": text,
                            }));
                        }
                    }
                }
            }
            "function_call" => {
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input = serde_json::from_str::<Value>(arguments).unwrap_or(json!({}));
                content_blocks.push(json!({
                    "type": "tool_use",
                    "id": item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                    "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                    "input": input,
                }));
                has_tool_calls = true;
            }
            _ => {}
        }
    }

    (content_blocks, has_tool_calls)
}

#[cfg(test)]
fn parse_openai_stream_events(
    events: &[String],
    response_bytes: usize,
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let mut acc = OpenAiStreamAccumulator::default();
    for event in events {
        acc.ingest_data(event)?;
    }
    acc.finalize(response_bytes)
}

#[derive(Default)]
struct AnthropicBlockAccum {
    block_type: String,
    id: String,
    name: String,
    text: String,
    input_json: String,
}

#[derive(Default)]
struct AnthropicStreamAccumulator {
    blocks: BTreeMap<usize, AnthropicBlockAccum>,
    stop_reason: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    saw_stop: bool,
    semantic_deltas: Vec<LlmStreamDelta>,
}

impl AnthropicStreamAccumulator {
    fn ingest_data(&mut self, data: &str) -> Result<Vec<LlmStreamDelta>, StreamParseFailure> {
        if data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }

        let event: Value = serde_json::from_str(data).map_err(|err| {
            StreamParseFailure::new(
                format!("parse Anthropic stream event: {err}"),
                self.semantic_output_seen(),
            )
        })?;
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        let mut deltas = Vec::new();

        match event_type {
            "message_start" => {
                if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                    self.input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(self.input_tokens);
                    self.cache_read_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(self.cache_read_input_tokens);
                    self.cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(self.cache_creation_input_tokens);
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let content_block = event.get("content_block").cloned().unwrap_or(json!({}));
                let block_type = content_block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut block = AnthropicBlockAccum {
                    block_type,
                    id: content_block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    name: content_block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    text: content_block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    input_json: String::new(),
                };
                if let Some(input) = content_block.get("input")
                    && !input.as_object().is_some_and(|obj| obj.is_empty())
                {
                    block.input_json = serde_json::to_string(input).unwrap_or_default();
                }
                if block.block_type == "tool_use" {
                    deltas.push(LlmStreamDelta::tool(
                        (!block.id.is_empty()).then(|| block.id.clone()),
                        (!block.name.is_empty()).then(|| block.name.clone()),
                        None,
                        self.accumulated_text_chars(),
                    ));
                }
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = event.get("delta").cloned().unwrap_or(json!({}));
                let block = self.blocks.entry(index).or_default();
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        if !text.is_empty() {
                            block.block_type = "text".to_string();
                            block.text.push_str(text);
                            let accumulated = self.accumulated_text_chars();
                            deltas.push(LlmStreamDelta::text(text, accumulated));
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !partial.is_empty() {
                            block.block_type = "tool_use".to_string();
                            block.input_json.push_str(partial);
                            deltas.push(LlmStreamDelta::tool(
                                (!block.id.is_empty()).then(|| block.id.clone()),
                                (!block.name.is_empty()).then(|| block.name.clone()),
                                Some(partial.to_string()),
                                self.accumulated_text_chars(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(stop_reason) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = stop_reason.to_string();
                }
                if let Some(usage) = event.get("usage") {
                    self.output_tokens = usage
                        .get("output_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(self.output_tokens);
                }
            }
            "message_stop" => {
                self.saw_stop = true;
            }
            "error" => {
                return Err(StreamParseFailure::new(
                    format!("Anthropic stream error event: {event}"),
                    self.semantic_output_seen(),
                ));
            }
            _ => {}
        }

        self.semantic_deltas.extend(deltas.clone());
        Ok(deltas)
    }

    fn accumulated_text_chars(&self) -> usize {
        self.blocks
            .values()
            .map(|block| block.text.chars().count())
            .sum()
    }

    fn semantic_output_seen(&self) -> bool {
        self.accumulated_text_chars() > 0
            || self
                .blocks
                .values()
                .any(|block| block.block_type == "tool_use" && !block.input_json.is_empty())
            || self.semantic_deltas.iter().any(LlmStreamDelta::is_semantic)
    }

    fn finalize(self, response_bytes: usize) -> Result<ParsedProviderStream, StreamParseFailure> {
        if !self.saw_stop {
            return Err(StreamParseFailure::new(
                "Anthropic SSE stream ended before message_stop",
                self.semantic_output_seen(),
            ));
        }

        let mut content = Vec::<Value>::new();
        let mut has_tool_calls = false;
        for block in self.blocks.values() {
            match block.block_type.as_str() {
                "text" => {
                    if !block.text.is_empty() {
                        content.push(json!({
                            "type": "text",
                            "text": block.text,
                        }));
                    }
                }
                "tool_use" => {
                    let input = if block.input_json.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str::<Value>(&block.input_json).unwrap_or(json!({}))
                    };
                    content.push(json!({
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": input,
                    }));
                    has_tool_calls = true;
                }
                _ => {}
            }
        }

        Ok(ParsedProviderStream {
            content: Value::Array(content),
            stop_reason: if !self.stop_reason.is_empty() {
                self.stop_reason
            } else if has_tool_calls {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            },
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            response_bytes,
            semantic_deltas: self.semantic_deltas,
            completed: true,
            // The Anthropic Messages stream carries no token ids or logprobs.
            token_signals: None,
        })
    }
}

#[cfg(test)]
fn parse_anthropic_stream_events(
    events: &[String],
    response_bytes: usize,
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let mut acc = AnthropicStreamAccumulator::default();
    for event in events {
        acc.ingest_data(event)?;
    }
    acc.finalize(response_bytes)
}

// The OpenRouter-specific path below is NOT the live one. `call_provider`
// dispatches "openrouter" to `call_openai_compatible_chat`, which uses the
// shared `openai_chat_wire` conversions and accumulator. Everything from here
// to `convert_tools_to_openrouter` is unreachable — the compiler says so on
// every build ("never used" / "never constructed"), and those warnings are left
// standing deliberately rather than silenced with `#[allow(dead_code)]`.
//
// It is kept, not deleted, because whether this copy has a future is
// nerdsane/temperpaw#459's call, not this branch's. It is still maintained to
// the same invariants — a reader who mistakes it for the live path must not
// find a stale rule in it.

#[derive(Default)]
struct OpenRouterToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct OpenRouterStreamAccumulator {
    text: String,
    tool_calls: BTreeMap<usize, OpenRouterToolCallAccum>,
    finish_reason: String,
    input_tokens: i64,
    output_tokens: i64,
    saw_done: bool,
    semantic_deltas: Vec<LlmStreamDelta>,
    token_signals: Option<Value>,
    response_id: String,
}

impl OpenRouterStreamAccumulator {
    fn ingest_data(&mut self, data: &str) -> Result<Vec<LlmStreamDelta>, StreamParseFailure> {
        if data.trim() == "[DONE]" {
            self.saw_done = true;
            return Ok(Vec::new());
        }

        let event: Value = serde_json::from_str(data).map_err(|err| {
            StreamParseFailure::new(
                format!("parse OpenRouter stream event: {err}"),
                self.semantic_output_seen(),
            )
        })?;
        let mut deltas = Vec::new();

        if self.response_id.is_empty()
            && let Some(id) = event.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            self.response_id = id.to_string();
        }

        if let Some(usage) = event.get("usage") {
            self.input_tokens = usage
                .get("prompt_tokens")
                .and_then(Value::as_i64)
                .or_else(|| usage.get("input_tokens").and_then(Value::as_i64))
                .unwrap_or(self.input_tokens);
            self.output_tokens = usage
                .get("completion_tokens")
                .and_then(Value::as_i64)
                .or_else(|| usage.get("output_tokens").and_then(Value::as_i64))
                .unwrap_or(self.output_tokens);
        }

        // One event contributes each signal once — see `event_token_signals`.
        if let Some(source) = event_token_signals(
            event.get("usage"),
            event
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first()),
        ) {
            merge_token_signals(&mut self.token_signals, &source);
        }

        if let Some(choice) = event
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        {
            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish_reason = finish_reason.to_string();
            }
            if let Some(delta) = choice.get("delta") {
                if let Some(text) = delta.get("content").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    self.text.push_str(text);
                    deltas.push(LlmStreamDelta::text(text, self.text.chars().count()));
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        let index =
                            tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let accum = self.tool_calls.entry(index).or_default();
                        if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                            accum.id = id.to_string();
                        }
                        if let Some(name) = tool_call
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                        {
                            accum.name = name.to_string();
                        }
                        let args_delta = tool_call
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if !args_delta.is_empty() {
                            accum.arguments.push_str(args_delta);
                        }
                        deltas.push(LlmStreamDelta::tool(
                            (!accum.id.is_empty()).then(|| accum.id.clone()),
                            (!accum.name.is_empty()).then(|| accum.name.clone()),
                            (!args_delta.is_empty()).then(|| args_delta.to_string()),
                            self.text.chars().count(),
                        ));
                    }
                }
            }
        }

        self.semantic_deltas.extend(deltas.clone());
        Ok(deltas)
    }

    fn semantic_output_seen(&self) -> bool {
        !self.text.is_empty()
            || !self.tool_calls.is_empty()
            || self.semantic_deltas.iter().any(LlmStreamDelta::is_semantic)
    }

    fn finalize(self, response_bytes: usize) -> Result<ParsedProviderStream, StreamParseFailure> {
        if !self.saw_done && self.finish_reason.is_empty() {
            return Err(StreamParseFailure::new(
                "OpenRouter SSE stream ended before [DONE] or finish_reason",
                self.semantic_output_seen(),
            ));
        }

        let mut content = Vec::<Value>::new();
        if !self.text.is_empty() {
            content.push(json!({
                "type": "text",
                "text": self.text,
            }));
        }
        for (idx, tool_call) in &self.tool_calls {
            let input = if tool_call.arguments.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str::<Value>(&tool_call.arguments).unwrap_or(json!({}))
            };
            content.push(json!({
                "type": "tool_use",
                "id": if tool_call.id.is_empty() {
                    synthetic_tool_call_id(&self.response_id, "or_tool", idx + 1)
                } else {
                    tool_call.id.clone()
                },
                "name": if tool_call.name.is_empty() { "unknown_tool".to_string() } else { tool_call.name.clone() },
                "input": input,
            }));
        }

        Ok(ParsedProviderStream {
            content: Value::Array(content),
            stop_reason: if !self.tool_calls.is_empty() {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            },
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            response_bytes,
            semantic_deltas: self.semantic_deltas,
            completed: true,
            token_signals: self.token_signals,
        })
    }
}

#[cfg(test)]
fn parse_openrouter_stream_events(
    events: &[String],
    response_bytes: usize,
) -> Result<ParsedProviderStream, StreamParseFailure> {
    let mut acc = OpenRouterStreamAccumulator::default();
    for event in events {
        acc.ingest_data(event)?;
    }
    acc.finalize(response_bytes)
}

struct StreamingHttpResponse {
    status: u16,
    body: String,
    response_bytes: usize,
}

#[cfg(target_arch = "wasm32")]
fn response_header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[cfg(not(target_arch = "wasm32"))]
fn feed_sse_or_json_lines<F>(body: &str, mut on_data: F) -> Result<(), StreamParseFailure>
where
    F: FnMut(&str) -> Result<(), StreamParseFailure>,
{
    if body
        .lines()
        .any(|line| line.trim_start().starts_with("data:"))
    {
        let events = collect_sse_data_events(&[body.as_bytes()])
            .map_err(|err| StreamParseFailure::new(err, false))?;
        for event in events {
            on_data(&event)?;
        }
        return Ok(());
    }

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        on_data(line)?;
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn post_sse_streaming<F>(
    _ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    body: &str,
    mut on_data: F,
) -> Result<StreamingHttpResponse, StreamParseFailure>
where
    F: FnMut(&str) -> Result<(), StreamParseFailure>,
{
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (mut request_body, mut response_body, fetch_head) =
        temper_wasm_sdk::http_stream::streaming_call("POST", api_url, &header_refs).map_err(
            |err| StreamParseFailure::new(format!("streaming_call begin: {err}"), false),
        )?;

    for chunk in body.as_bytes().chunks(LLM_STREAM_REQUEST_CHUNK_BYTES) {
        request_body.write_all_chunk(chunk).map_err(|err| {
            StreamParseFailure::new(format!("streaming request write: {err}"), false)
        })?;
    }
    request_body.finish().map_err(|err| {
        StreamParseFailure::new(format!("streaming request finish: {err}"), false)
    })?;

    let head = fetch_head()
        .map_err(|err| StreamParseFailure::new(format!("streaming response head: {err}"), false))?;
    let success = (200..300).contains(&head.status);
    let mut response_bytes = 0usize;
    let mut body_text = String::new();
    let mut decoder = SseDataDecoder::default();
    let mut buf = vec![0u8; LLM_STREAM_READ_BUFFER_BYTES];

    loop {
        let n = match response_body.read_next_chunk(&mut buf) {
            Ok(Some(n)) => n,
            Ok(None) => break,
            Err(err) => {
                return Err(StreamParseFailure::new(
                    format!("streaming response read: {err}"),
                    false,
                ));
            }
        };
        response_bytes += n;
        if success {
            for event in decoder.push_chunk(&buf[..n]) {
                on_data(&event)?;
            }
        } else {
            body_text.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    }

    if success {
        for event in decoder.finish() {
            on_data(&event)?;
        }
    } else if head.status == 0
        && let Some(stream_error) = response_header_value(&head.headers, "x-temper-stream-error")
        && body_text.is_empty()
    {
        body_text.push_str(stream_error);
    }

    let _ = response_body.close();
    Ok(StreamingHttpResponse {
        status: head.status,
        body: body_text,
        response_bytes,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn post_sse_streaming<F>(
    ctx: &Context,
    api_url: &str,
    headers: &[(String, String)],
    body: &str,
    on_data: F,
) -> Result<StreamingHttpResponse, StreamParseFailure>
where
    F: FnMut(&str) -> Result<(), StreamParseFailure>,
{
    let resp = ctx
        .http_call("POST", api_url, headers, body)
        .map_err(|err| StreamParseFailure::new(err, false))?;
    if (200..300).contains(&resp.status) {
        feed_sse_or_json_lines(&resp.body, on_data)?;
    }
    Ok(StreamingHttpResponse {
        status: resp.status,
        response_bytes: resp.body.len(),
        body: resp.body,
    })
}

struct LlmLiveProgress<'a> {
    ctx: &'a Context,
    temper_api_url: &'a str,
    tenant: &'a str,
    provider: &'a str,
    model: &'a str,
    sequence: u64,
    saw_semantic_output: bool,
    semantic_output_bytes: usize,
    last_progress_ms: Option<i64>,
    last_progress_bytes: usize,
}

impl<'a> LlmLiveProgress<'a> {
    fn new(
        ctx: &'a Context,
        temper_api_url: &'a str,
        tenant: &'a str,
        provider: &'a str,
        model: &'a str,
    ) -> Self {
        Self {
            ctx,
            temper_api_url,
            tenant,
            provider,
            model,
            sequence: 0,
            saw_semantic_output: false,
            semantic_output_bytes: 0,
            last_progress_ms: None,
            last_progress_bytes: 0,
        }
    }

    fn saw_semantic_output(&self) -> bool {
        self.saw_semantic_output
    }

    fn emit_deltas(&mut self, deltas: &[LlmStreamDelta]) {
        for delta in deltas {
            self.emit_delta(delta);
        }
    }

    fn emit_delta(&mut self, delta: &LlmStreamDelta) {
        if !delta.is_semantic() {
            return;
        }

        let was_first_semantic_output = !self.saw_semantic_output;
        self.saw_semantic_output = true;
        self.sequence += 1;
        self.semantic_output_bytes += delta.delta_text.len();
        if let Some(tool_delta) = &delta.tool_arguments_delta {
            self.semantic_output_bytes += tool_delta.len();
        }

        let mut payload = json!({
            "kind": "llm_delta",
            "provider": self.provider,
            "model": self.model,
            "sequence": self.sequence,
            "delta_text": delta.delta_text,
            "accumulated_text_chars": delta.accumulated_text_chars,
        });
        if let Some(obj) = payload.as_object_mut() {
            if let Some(tool_call_id) = &delta.tool_call_id {
                obj.insert("tool_call_id".to_string(), json!(tool_call_id));
            }
            if let Some(tool_name) = &delta.tool_name {
                obj.insert("tool_name".to_string(), json!(tool_name));
            }
            if let Some(tool_arguments_delta) = &delta.tool_arguments_delta {
                obj.insert(
                    "tool_arguments_delta".to_string(),
                    json!(tool_arguments_delta),
                );
            }
        }

        if let Err(err) = self.ctx.emit_progress(&payload) {
            self.ctx.log(
                "warn",
                &format!("session_turn: failed to emit llm_delta progress event: {err}"),
            );
        }

        let now = Context::get_time_millis();
        let should_dispatch = was_first_semantic_output
            || self
                .last_progress_ms
                .is_some_and(|last| now - last >= LLM_STREAM_PROGRESS_INTERVAL_MS)
            || self
                .semantic_output_bytes
                .saturating_sub(self.last_progress_bytes)
                >= LLM_STREAM_PROGRESS_BYTES;
        if should_dispatch {
            if let Err(err) = send_progress(self.ctx, self.temper_api_url, self.tenant) {
                self.ctx.log(
                    "warn",
                    &format!("session_turn: failed to dispatch ProgressMade for LLM stream: {err}"),
                );
            }
            self.last_progress_ms = Some(now);
            self.last_progress_bytes = self.semantic_output_bytes;
        }
    }
}

/// Max output tokens passed to both Anthropic and OpenRouter. Kept as a
/// constant so the value is reusable as a `gen_ai.request.max_tokens`
/// span-hint attribute without drifting from the body payload.
const LLM_MAX_TOKENS: u32 = 16384;

/// Upper bound for the `gen_ai.prompt` span-hint value emitted from this
/// module. The host truncates at 20 KB; we cap slightly lower so our
/// `[truncated]` suffix lines up with the guest-level cut rather than the
/// host-level one (easier for operators reading traces to spot). Values
/// are truncated on a UTF-8 boundary.
const LLM_PROMPT_ATTR_MAX_BYTES: usize = 18 * 1024;
const LLM_COMPLETION_ATTR_MAX_BYTES: usize = 18 * 1024;
const LLM_ATTR_TRUNCATED_SUFFIX: &str = "…[truncated]";

fn truncate_llm_span_attr(raw: &str, max_bytes: usize) -> String {
    if raw.len() <= max_bytes {
        return raw.to_string();
    }
    let mut cut = max_bytes;
    while cut > 0 && !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &raw[..cut], LLM_ATTR_TRUNCATED_SUFFIX)
}

/// Serialize `system_prompt` + `messages` as a compact JSON object suitable
/// for the `gen_ai.prompt` span attribute, truncating at a UTF-8 boundary
/// if the payload is larger than [`LLM_PROMPT_ATTR_MAX_BYTES`]. The shape
/// (`{"system": ..., "messages": ...}`) mirrors what DD LLM Obs parsers
/// expect for OpenAI-style payloads and is also readable for Anthropic
/// (whose native shape has system separate).
fn format_gen_ai_prompt_attr(system_prompt: &str, messages: &[Value]) -> String {
    let payload = if system_prompt.is_empty() {
        json!({ "messages": messages })
    } else {
        json!({
            "system": system_prompt,
            "messages": messages,
        })
    };
    let raw = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    truncate_llm_span_attr(&raw, LLM_PROMPT_ATTR_MAX_BYTES)
}

fn format_gen_ai_completion_attr(completion: &Value) -> String {
    truncate_llm_span_attr(
        &stringify_content(completion),
        LLM_COMPLETION_ATTR_MAX_BYTES,
    )
}

#[allow(dead_code)]
fn append_llm_span_hint_headers(
    headers: &mut Vec<(String, String)>,
    provider: &str,
    model: &str,
    temperature: f64,
    max_tokens: u32,
    system_prompt: &str,
    messages: &[Value],
    session_id: &str,
    completion_pointer: &str,
) {
    headers.push((
        "X-Temper-Span-Name".to_string(),
        "tool.llm_call".to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.operation.name".to_string(),
        "chat".to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-dd_llmobs_enabled".to_string(),
        "false".to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.provider.name".to_string(),
        provider.to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.system".to_string(),
        provider.to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.request.model".to_string(),
        model.to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.response.model".to_string(),
        model.to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.request.temperature".to_string(),
        format!("{temperature}"),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.request.max_tokens".to_string(),
        max_tokens.to_string(),
    ));
    if !session_id.trim().is_empty() {
        headers.push((
            "X-Temper-Span-Attr-gen_ai.conversation.id".to_string(),
            session_id.to_string(),
        ));
        headers.push((
            "X-Temper-Span-Attr-session_id".to_string(),
            session_id.to_string(),
        ));
    }
    headers.push((
        "X-Temper-Span-Attr-tool.name".to_string(),
        "provider_caller".to_string(),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.system_instructions".to_string(),
        build_gen_ai_system_instructions(system_prompt),
    ));
    headers.push((
        "X-Temper-Span-Attr-gen_ai.input.messages".to_string(),
        build_gen_ai_input_messages(messages),
    ));
    // Keep the legacy prompt/completion hints while Datadog LLMObs rolls out
    // GenAI semantic convention mappings across every org.
    headers.push((
        "X-Temper-Span-Attr-gen_ai.prompt".to_string(),
        format_gen_ai_prompt_attr(system_prompt, messages),
    ));
    headers.push((
        "X-Temper-Span-Capture-Response-gen_ai.completion".to_string(),
        completion_pointer.to_string(),
    ));
}

fn llm_guest_span_attributes(
    provider: &str,
    model: &str,
    temperature: f64,
    max_tokens: u32,
    system_prompt: &str,
    messages: &[Value],
    session_id: &str,
) -> Value {
    let mut attrs = json!({
        "gen_ai.operation.name": "chat",
        "gen_ai.provider.name": provider,
        "gen_ai.system": provider,
        "gen_ai.request.model": model,
        "gen_ai.response.model": model,
        "gen_ai.request.temperature": temperature,
        "gen_ai.request.max_tokens": max_tokens,
        "tool.name": "provider_caller",
        "gen_ai.system_instructions": build_gen_ai_system_instructions(system_prompt),
        "gen_ai.input.messages": build_gen_ai_input_messages(messages),
        "gen_ai.prompt": format_gen_ai_prompt_attr(system_prompt, messages),
    });
    if !session_id.trim().is_empty() {
        attrs["gen_ai.conversation.id"] = json!(session_id);
        attrs["session_id"] = json!(session_id);
    }
    attrs
}

fn start_llm_guest_span(
    ctx: &Context,
    provider: &str,
    model: &str,
    temperature: f64,
    max_tokens: u32,
    system_prompt: &str,
    messages: &[Value],
    session_id: &str,
) -> Option<WasmSpan> {
    let attrs = llm_guest_span_attributes(
        provider,
        model,
        temperature,
        max_tokens,
        system_prompt,
        messages,
        session_id,
    );
    match ctx.start_span_with_kind("tool.llm_call", Some("client"), &attrs) {
        Ok(span) => Some(span),
        Err(err) => {
            ctx.log(
                "warn",
                &format!("provider_caller: failed to start LLM guest span: {err}"),
            );
            None
        }
    }
}

fn finish_llm_guest_span_success(
    ctx: &Context,
    span: &mut Option<WasmSpan>,
    provider: &str,
    model: &str,
    stop_reason: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    response_bytes: usize,
    completion: &Value,
) {
    if let Some(span) = span.take() {
        let attrs = llm_success_span_attributes(
            provider,
            model,
            stop_reason,
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            response_bytes,
            completion,
        );
        let output_event_attrs = json!({
            "gen_ai.output.messages": attrs
                .get("gen_ai.output.messages")
                .cloned()
                .unwrap_or_else(|| json!("[]")),
        });
        log_llm_span_export_result(
            ctx,
            "add gen_ai.client.inference.operation.details",
            span.add_event(
                "gen_ai.client.inference.operation.details",
                &output_event_attrs,
            ),
        );
        log_llm_span_export_result(
            ctx,
            "add llm.response",
            span.add_event("llm.response", &attrs),
        );
        log_llm_span_export_result(ctx, "set success attributes", span.set_attributes(&attrs));
        log_llm_span_export_result(ctx, "end success", span.end_ok(&json!({})));
    } else {
        ctx.log(
            "warn",
            "provider_caller: LLM span success export skipped because no active span was available",
        );
    }
}

fn log_llm_span_export_result(ctx: &Context, operation: &str, result: Result<(), String>) {
    if let Err(err) = result {
        ctx.log(
            "warn",
            &format!("provider_caller: LLM span export failed operation={operation} error={err}"),
        );
    }
}

fn llm_success_span_attributes(
    provider: &str,
    model: &str,
    stop_reason: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    response_bytes: usize,
    completion: &Value,
) -> Value {
    json!({
        "gen_ai.provider.name": provider,
        "gen_ai.response.model": model,
        "gen_ai.response.finish_reasons": stop_reason,
        "gen_ai.usage.input_tokens": input_tokens,
        "gen_ai.usage.output_tokens": output_tokens,
        "gen_ai.usage.cache_read_input_tokens": cache_read_input_tokens,
        "gen_ai.usage.cache_creation_input_tokens": cache_creation_input_tokens,
        "http.response.body.size": response_bytes,
        "gen_ai.output.messages": build_gen_ai_output_messages(completion, stop_reason),
        "gen_ai.completion": format_gen_ai_completion_attr(completion),
    })
}

fn finish_llm_guest_span_error(span: &mut Option<WasmSpan>, error_type: &str, error_message: &str) {
    if let Some(span) = span.take() {
        let _ = span.end_error(error_type, error_message, &json!({}));
    }
}

/// Format a post-response usage log line using OpenTelemetry
/// `gen_ai.*` semconv keys so DD's grok parser indexes them as
/// structured attributes. The `wasm_guest` tracing bridge attaches
/// the current span/trace IDs to the log, giving DD APM a clickable
/// correlation between the `tool.llm_call.*` span and these usage
/// numbers even though they cannot be set as span attributes post-hoc
/// via hint headers (ADR-0037 Fix C4).
fn format_gen_ai_usage_log(
    provider: &str,
    model: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
) -> String {
    format!(
        "session_turn: usage \
         gen_ai.system={provider} \
         gen_ai.request.model={model} \
         gen_ai.usage.input_tokens={input_tokens} \
         gen_ai.usage.output_tokens={output_tokens} \
         gen_ai.usage.cache_read_input_tokens={cache_read_input_tokens} \
         gen_ai.usage.cache_creation_input_tokens={cache_creation_input_tokens}"
    )
}

fn call_anthropic(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    api_key: &str,
    api_url: &str,
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    anthropic_auth_mode: &str,
    temperature: f64,
) -> Result<LlmResponse, String> {
    // Detect OAuth token (sk-ant-oat-*) vs standard API key
    let is_oauth = detect_anthropic_oauth_mode(api_key, anthropic_auth_mode);

    // OAuth tokens enforce a fixed system prompt when tools are present.
    // Custom system instructions are prepended to the first user message instead.
    let (effective_system, effective_messages) = if is_oauth {
        let oauth_system = "You are Claude Code, Anthropic's official CLI for Claude.".to_string();
        let mut msgs = messages.to_vec();
        if !system_prompt.is_empty() {
            if let Some(first) = msgs.first_mut() {
                if let Some(content) = first.get("content").and_then(|v| v.as_str()) {
                    let combined = format!("[System instructions: {system_prompt}]\n\n{content}");
                    first["content"] = json!(combined);
                }
            }
        }
        (oauth_system, msgs)
    } else {
        (system_prompt.to_string(), messages.to_vec())
    };

    let mut body = json!({
        "model": model,
        "max_tokens": LLM_MAX_TOKENS,
        "messages": effective_messages,
        "temperature": temperature,
        "stream": true,
    });

    if !effective_system.is_empty() {
        body["system"] = json!([{
            "type": "text",
            "text": effective_system,
            "cache_control": {"type": "ephemeral"}
        }]);
    }

    // Add cache_control breakpoints to up to 2 recent user messages
    if let Some(msgs_arr) = body.get_mut("messages").and_then(|v| v.as_array_mut()) {
        let mut user_indices: Vec<usize> = Vec::new();
        for (i, m) in msgs_arr.iter().enumerate() {
            if m.get("role").and_then(|v| v.as_str()) == Some("user") {
                user_indices.push(i);
            }
        }
        // Take last 2 user message indices
        let breakpoints: Vec<usize> = user_indices.into_iter().rev().take(2).collect();
        for idx in breakpoints {
            let msg = &mut msgs_arr[idx];
            if let Some(content) = msg.get_mut("content") {
                if let Some(arr) = content.as_array_mut() {
                    // Array content — add cache_control to last block
                    if let Some(last) = arr.last_mut() {
                        last["cache_control"] = json!({"type": "ephemeral"});
                    }
                } else if let Some(text) = content.as_str().map(|s| s.to_string()) {
                    // String content — convert to array format with cache_control
                    msg["content"] = json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": {"type": "ephemeral"}
                    }]);
                }
            }
        }
    }

    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }

    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("JSON serialize error: {e}"))?;

    ctx.log(
        "info",
        &format!(
            "session_turn: calling Anthropic API, model={model}, oauth={is_oauth}, messages={}, url={api_url}",
            messages.len(),
        ),
    );

    // Build auth headers — OAuth tokens use Bearer + beta header
    let headers = if is_oauth {
        vec![
            ("authorization".to_string(), format!("Bearer {api_key}")),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            (
                "anthropic-beta".to_string(),
                "oauth-2025-04-20,computer-use-2025-01-24,prompt-caching-2024-07-31".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
            ("user-agent".to_string(), "claude-cli/2.1.75".to_string()),
            ("x-app".to_string(), "cli".to_string()),
        ]
    } else {
        vec![
            ("x-api-key".to_string(), api_key.to_string()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            (
                "anthropic-beta".to_string(),
                "prompt-caching-2024-07-31".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
        ]
    };
    let mut llm_span = start_llm_guest_span(
        ctx,
        "anthropic",
        model,
        temperature,
        LLM_MAX_TOKENS,
        &effective_system,
        &effective_messages,
        &ctx.entity_id,
    );

    // Retry on transient API errors only until the stream emits visible output.
    // Once the user has seen a semantic delta, replaying the call would duplicate
    // live output, so a midstream failure is surfaced clearly instead.
    let overall_start_ms = Context::get_time_millis();
    let mut last_err = String::new();
    let mut parsed_stream = None;
    let mut attempts_used: u32 = 0;
    let mut live_progress = LlmLiveProgress::new(ctx, temper_api_url, tenant, "anthropic", model);
    for attempt in 0..LLM_MAX_ATTEMPTS {
        let attempt_num = attempt + 1;
        attempts_used = attempt_num;
        if attempt > 0 {
            ctx.log(
                "warn",
                &format!(
                    "session_turn: anthropic retrying (attempt {attempt_num}/5), last error: {last_err}"
                ),
            );
        }
        ctx.log(
            "info",
            &format_llm_attempt_start_log(
                "anthropic",
                model,
                attempt_num,
                LLM_MAX_ATTEMPTS,
                Context::get_time_millis() - overall_start_ms,
            ),
        );
        let attempt_start_ms = Context::get_time_millis();
        let mut accumulator = AnthropicStreamAccumulator::default();
        let stream_result = post_sse_streaming(ctx, api_url, &headers, &body_str, |data| {
            let deltas = accumulator.ingest_data(data)?;
            live_progress.emit_deltas(&deltas);
            Ok(())
        });
        match stream_result {
            Ok(r) if r.status == 200 => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "anthropic",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("anthropic", attempt_num, elapsed),
                    );
                }
                match accumulator.finalize(r.response_bytes) {
                    Ok(parsed) => {
                        parsed_stream = Some(parsed);
                        break;
                    }
                    Err(err) => {
                        last_err = err.to_string();
                        let visible =
                            err.semantic_output_seen || live_progress.saw_semantic_output();
                        if should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                            ctx.log(
                                "warn",
                                &format!("session_turn: Anthropic stream parse failed before visible output, will retry: {last_err}"),
                            );
                            continue;
                        }
                        let error = format!(
                            "Anthropic stream failed after visible output or final attempt: {last_err}"
                        );
                        finish_llm_guest_span_error(&mut llm_span, "stream_parse_error", &error);
                        return Err(error);
                    }
                }
            }
            Ok(r) if r.status == 500 || r.status == 529 => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "anthropic",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("anthropic", attempt_num, elapsed),
                    );
                }
                last_err = format!("HTTP {}: {}", r.status, &r.body[..r.body.len().min(200)]);
                if live_progress.saw_semantic_output() {
                    let error = format!(
                        "Anthropic stream returned transient HTTP {} after visible output: {}",
                        r.status,
                        &r.body[..r.body.len().min(500)]
                    );
                    finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                    return Err(error);
                }
                continue;
            }
            Ok(r) if r.status == 400 && r.body.contains("\"message\":\"Error\"") => {
                // Transient 400 with vague error message — retry
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "anthropic",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("anthropic", attempt_num, elapsed),
                    );
                }
                last_err = format!("HTTP 400 (transient): {}", &r.body[..r.body.len().min(200)]);
                if live_progress.saw_semantic_output() {
                    let error = format!(
                        "Anthropic stream returned transient HTTP 400 after visible output: {}",
                        &r.body[..r.body.len().min(500)]
                    );
                    finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                    return Err(error);
                }
                continue;
            }
            Ok(r) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "anthropic",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                let total_elapsed = Context::get_time_millis() - overall_start_ms;
                ctx.log(
                    "info",
                    &format_llm_complete_log(
                        "anthropic",
                        model,
                        attempts_used,
                        total_elapsed,
                        "non_retriable_http_error",
                    ),
                );
                let error = format!(
                    "Anthropic API returned {}: {}",
                    r.status,
                    &r.body[..r.body.len().min(500)]
                );
                finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                return Err(error);
            }
            Err(e) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "warn",
                    &format!(
                        "session_turn: anthropic attempt {attempt_num} stream error elapsed_ms={elapsed} err={e}"
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("anthropic", attempt_num, elapsed),
                    );
                }
                last_err = e.to_string();
                let visible = e.semantic_output_seen || live_progress.saw_semantic_output();
                if !should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                    let error = format!(
                        "Anthropic stream failed after visible output or final attempt: {last_err}"
                    );
                    finish_llm_guest_span_error(&mut llm_span, "stream_error", &error);
                    return Err(error);
                }
                continue;
            }
        }
    }
    let parsed = match parsed_stream {
        Some(parsed) => parsed,
        None => {
            let total_elapsed = Context::get_time_millis() - overall_start_ms;
            ctx.log(
                "warn",
                &format_llm_complete_log(
                    "anthropic",
                    model,
                    attempts_used,
                    total_elapsed,
                    "exhausted_retries",
                ),
            );
            let error = format!("Anthropic API failed after 5 attempts: {last_err}");
            finish_llm_guest_span_error(&mut llm_span, "exhausted_retries", &error);
            return Err(error);
        }
    };
    ctx.log(
        "info",
        &format_llm_complete_log(
            "anthropic",
            model,
            attempts_used,
            Context::get_time_millis() - overall_start_ms,
            "success",
        ),
    );

    ctx.log(
        "info",
        &format_gen_ai_usage_log(
            "anthropic",
            model,
            parsed.input_tokens,
            parsed.output_tokens,
            parsed.cache_read_input_tokens,
            parsed.cache_creation_input_tokens,
        ),
    );

    finish_llm_guest_span_success(
        ctx,
        &mut llm_span,
        "anthropic",
        model,
        &parsed.stop_reason,
        parsed.input_tokens,
        parsed.output_tokens,
        parsed.cache_read_input_tokens,
        parsed.cache_creation_input_tokens,
        parsed.response_bytes,
        &parsed.content,
    );

    Ok(parsed.into_llm_response(body_str.len()))
}

/// Call OpenRouter Chat Completions API (OpenAI-compatible schema).
fn call_openai_compatible_chat(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    provider: &str,
    api_key: &str,
    api_url: &str,
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    site_url: &str,
    app_name: &str,
    extra_headers: &[(String, String)],
    temperature: f64,
    provider_options_json: &str,
) -> Result<LlmResponse, String> {
    let body = build_chat_completion_body(
        model,
        system_prompt,
        messages,
        tools,
        LLM_MAX_TOKENS as i64,
        temperature,
        true,
        true,
        provider_options_json,
    )?;
    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("JSON serialize error: {e}"))?;

    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "text/event-stream".to_string()),
    ];
    if !api_key.trim().is_empty() {
        headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
    }
    if provider == "openrouter" {
        if !site_url.trim().is_empty() {
            headers.push(("HTTP-Referer".to_string(), site_url.trim().to_string()));
        }
        if !app_name.trim().is_empty() {
            headers.push(("X-Title".to_string(), app_name.trim().to_string()));
        }
    }
    headers.extend(extra_headers.iter().cloned());

    let chat_messages = convert_messages_to_chat(system_prompt, messages);
    let mut llm_span = start_llm_guest_span(
        ctx,
        provider,
        model,
        temperature,
        LLM_MAX_TOKENS,
        system_prompt,
        &chat_messages,
        &ctx.entity_id,
    );

    ctx.log(
        "info",
        &format!(
            "session_turn: calling OpenAI-compatible chat provider={provider}, model={model}, messages={}, url={api_url}",
            messages.len(),
        ),
    );

    let overall_start_ms = Context::get_time_millis();
    let mut last_err = String::new();
    let mut parsed_stream = None;
    let mut attempts_used: u32 = 0;
    let mut live_progress = LlmLiveProgress::new(ctx, temper_api_url, tenant, provider, model);
    for attempt in 0..LLM_MAX_ATTEMPTS {
        let attempt_num = attempt + 1;
        attempts_used = attempt_num;
        if attempt > 0 {
            ctx.log(
                "warn",
                &format!(
                    "session_turn: {provider} retrying (attempt {attempt_num}/{LLM_MAX_ATTEMPTS}), last error: {last_err}"
                ),
            );
        }
        ctx.log(
            "info",
            &format_llm_attempt_start_log(
                provider,
                model,
                attempt_num,
                LLM_MAX_ATTEMPTS,
                Context::get_time_millis() - overall_start_ms,
            ),
        );
        let attempt_start_ms = Context::get_time_millis();
        let mut accumulator = ChatCompletionStreamAccumulator::default();
        let stream_result = post_sse_streaming(ctx, api_url, &headers, &body_str, |data| {
            let deltas = accumulator.ingest_data(data).map_err(chat_stream_error)?;
            let llm_deltas = chat_deltas_to_llm(deltas);
            live_progress.emit_deltas(&llm_deltas);
            Ok(())
        });
        match stream_result {
            Ok(r) if r.status == 200 => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        provider,
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint(provider, attempt_num, elapsed),
                    );
                }
                match accumulator.finalize(r.response_bytes) {
                    Ok(parsed) => {
                        parsed_stream = Some(parsed);
                        break;
                    }
                    Err(err) => {
                        last_err = err.to_string();
                        let visible =
                            err.semantic_output_seen || live_progress.saw_semantic_output();
                        if should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                            ctx.log(
                                "warn",
                                &format!(
                                    "session_turn: {provider} stream parse failed before visible output, will retry: {last_err}"
                                ),
                            );
                            continue;
                        }
                        let error = format!(
                            "{provider} stream failed after visible output or final attempt: {last_err}"
                        );
                        finish_llm_guest_span_error(&mut llm_span, "stream_parse_error", &error);
                        return Err(error);
                    }
                }
            }
            Ok(r) if matches!(r.status, 429 | 500 | 502 | 503 | 504) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        provider,
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint(provider, attempt_num, elapsed),
                    );
                }
                last_err = format!("HTTP {}: {}", r.status, &r.body[..r.body.len().min(200)]);
                if live_progress.saw_semantic_output() {
                    let error = format!(
                        "{provider} stream returned transient HTTP {} after visible output: {}",
                        r.status,
                        &r.body[..r.body.len().min(500)]
                    );
                    finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                    return Err(error);
                }
                continue;
            }
            Ok(r) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        provider,
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                let total_elapsed = Context::get_time_millis() - overall_start_ms;
                ctx.log(
                    "info",
                    &format_llm_complete_log(
                        provider,
                        model,
                        attempts_used,
                        total_elapsed,
                        "non_retriable_http_error",
                    ),
                );
                let error = format!(
                    "{provider} API returned {}: {}",
                    r.status,
                    &r.body[..r.body.len().min(500)]
                );
                finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                return Err(error);
            }
            Err(e) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "warn",
                    &format!(
                        "session_turn: {provider} attempt {attempt_num} stream error elapsed_ms={elapsed} err={e}"
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint(provider, attempt_num, elapsed),
                    );
                }
                last_err = e.to_string();
                let visible = e.semantic_output_seen || live_progress.saw_semantic_output();
                if !should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                    let error = format!(
                        "{provider} stream failed after visible output or final attempt: {last_err}"
                    );
                    finish_llm_guest_span_error(&mut llm_span, "stream_error", &error);
                    return Err(error);
                }
                continue;
            }
        }
    }

    let parsed = match parsed_stream {
        Some(parsed) => parsed,
        None => {
            let total_elapsed = Context::get_time_millis() - overall_start_ms;
            ctx.log(
                "warn",
                &format_llm_complete_log(
                    provider,
                    model,
                    attempts_used,
                    total_elapsed,
                    "exhausted_retries",
                ),
            );
            let error =
                format!("{provider} API failed after {LLM_MAX_ATTEMPTS} attempts: {last_err}");
            finish_llm_guest_span_error(&mut llm_span, "exhausted_retries", &error);
            return Err(error);
        }
    };
    ctx.log(
        "info",
        &format_llm_complete_log(
            provider,
            model,
            attempts_used,
            Context::get_time_millis() - overall_start_ms,
            "success",
        ),
    );

    ctx.log(
        "info",
        &format_gen_ai_usage_log(
            provider,
            model,
            parsed.input_tokens,
            parsed.output_tokens,
            0,
            0,
        ),
    );

    let content = Value::Array(parsed.content);
    finish_llm_guest_span_success(
        ctx,
        &mut llm_span,
        provider,
        model,
        &parsed.stop_reason,
        parsed.input_tokens,
        parsed.output_tokens,
        0,
        0,
        parsed.response_bytes,
        &content,
    );

    Ok(LlmResponse {
        content,
        stop_reason: parsed.stop_reason,
        input_tokens: parsed.input_tokens,
        output_tokens: parsed.output_tokens,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        request_bytes: body_str.len(),
        response_bytes: parsed.response_bytes,
        token_signals: parsed.token_signals,
    })
}

/// Unreachable: `call_provider` sends "openrouter" to
/// `call_openai_compatible_chat`. See the note above `OpenRouterToolCallAccum`.
fn call_openrouter(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    api_key: &str,
    api_url: &str,
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    site_url: &str,
    app_name: &str,
    temperature: f64,
) -> Result<LlmResponse, String> {
    let mut or_messages = Vec::<Value>::new();
    if !system_prompt.is_empty() {
        or_messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }
    or_messages.extend(convert_messages_to_openrouter(messages));

    let openai_tools = convert_tools_to_openrouter(tools);
    let mut body = json!({
        "model": model,
        "messages": or_messages,
        "max_tokens": LLM_MAX_TOKENS,
        "temperature": temperature,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !openai_tools.is_empty() {
        body["tools"] = json!(openai_tools);
        body["tool_choice"] = json!("auto");
    }

    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("JSON serialize error: {e}"))?;

    let mut headers = vec![
        ("authorization".to_string(), format!("Bearer {api_key}")),
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "text/event-stream".to_string()),
    ];
    if !site_url.trim().is_empty() {
        headers.push(("HTTP-Referer".to_string(), site_url.trim().to_string()));
    }
    if !app_name.trim().is_empty() {
        headers.push(("X-Title".to_string(), app_name.trim().to_string()));
    }
    let mut llm_span = start_llm_guest_span(
        ctx,
        "openrouter",
        model,
        temperature,
        LLM_MAX_TOKENS,
        system_prompt,
        &or_messages,
        &ctx.entity_id,
    );

    ctx.log(
        "info",
        &format!(
            "session_turn: calling OpenRouter API, model={model}, messages={}, url={api_url}",
            messages.len(),
        ),
    );

    // Per-attempt timing + hang hint per ADR-0037 Fix B. Streaming retries are
    // allowed only before any semantic delta has been emitted to observers.
    let overall_start_ms = Context::get_time_millis();
    let mut last_err = String::new();
    let mut parsed_stream = None;
    let mut attempts_used: u32 = 0;
    let mut live_progress = LlmLiveProgress::new(ctx, temper_api_url, tenant, "openrouter", model);
    for attempt in 0..LLM_MAX_ATTEMPTS {
        let attempt_num = attempt + 1;
        attempts_used = attempt_num;
        if attempt > 0 {
            ctx.log(
                "warn",
                &format!(
                    "session_turn: openrouter retrying (attempt {attempt_num}/5), last error: {last_err}"
                ),
            );
        }
        ctx.log(
            "info",
            &format_llm_attempt_start_log(
                "openrouter",
                model,
                attempt_num,
                LLM_MAX_ATTEMPTS,
                Context::get_time_millis() - overall_start_ms,
            ),
        );
        let attempt_start_ms = Context::get_time_millis();
        let mut accumulator = OpenRouterStreamAccumulator::default();
        let stream_result = post_sse_streaming(ctx, api_url, &headers, &body_str, |data| {
            let deltas = accumulator.ingest_data(data)?;
            live_progress.emit_deltas(&deltas);
            Ok(())
        });
        match stream_result {
            Ok(r) if r.status == 200 => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "openrouter",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("openrouter", attempt_num, elapsed),
                    );
                }
                match accumulator.finalize(r.response_bytes) {
                    Ok(parsed) => {
                        parsed_stream = Some(parsed);
                        break;
                    }
                    Err(err) => {
                        last_err = err.to_string();
                        let visible =
                            err.semantic_output_seen || live_progress.saw_semantic_output();
                        if should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                            ctx.log(
                                "warn",
                                &format!("session_turn: OpenRouter stream parse failed before visible output, will retry: {last_err}"),
                            );
                            continue;
                        }
                        let error = format!(
                            "OpenRouter stream failed after visible output or final attempt: {last_err}"
                        );
                        finish_llm_guest_span_error(&mut llm_span, "stream_parse_error", &error);
                        return Err(error);
                    }
                }
            }
            Ok(r) if matches!(r.status, 429 | 500 | 502 | 503 | 504) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "openrouter",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("openrouter", attempt_num, elapsed),
                    );
                }
                last_err = format!("HTTP {}: {}", r.status, &r.body[..r.body.len().min(200)]);
                if live_progress.saw_semantic_output() {
                    let error = format!(
                        "OpenRouter stream returned transient HTTP {} after visible output: {}",
                        r.status,
                        &r.body[..r.body.len().min(500)]
                    );
                    finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                    return Err(error);
                }
                continue;
            }
            Ok(r) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "info",
                    &format_llm_attempt_end_log(
                        "openrouter",
                        attempt_num,
                        elapsed,
                        r.status,
                        r.response_bytes,
                    ),
                );
                let total_elapsed = Context::get_time_millis() - overall_start_ms;
                ctx.log(
                    "info",
                    &format_llm_complete_log(
                        "openrouter",
                        model,
                        attempts_used,
                        total_elapsed,
                        "non_retriable_http_error",
                    ),
                );
                let error = format!(
                    "OpenRouter API returned {}: {}",
                    r.status,
                    &r.body[..r.body.len().min(500)]
                );
                finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                return Err(error);
            }
            Err(e) => {
                let elapsed = Context::get_time_millis() - attempt_start_ms;
                ctx.log(
                    "warn",
                    &format!(
                        "session_turn: openrouter attempt {attempt_num} stream error elapsed_ms={elapsed} err={e}"
                    ),
                );
                if should_emit_hang_hint(elapsed) {
                    ctx.log(
                        "warn",
                        &format_llm_hang_hint("openrouter", attempt_num, elapsed),
                    );
                }
                last_err = e.to_string();
                let visible = e.semantic_output_seen || live_progress.saw_semantic_output();
                if !should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                    let error = format!(
                        "OpenRouter stream failed after visible output or final attempt: {last_err}"
                    );
                    finish_llm_guest_span_error(&mut llm_span, "stream_error", &error);
                    return Err(error);
                }
                continue;
            }
        }
    }
    let parsed = match parsed_stream {
        Some(parsed) => parsed,
        None => {
            let total_elapsed = Context::get_time_millis() - overall_start_ms;
            ctx.log(
                "warn",
                &format_llm_complete_log(
                    "openrouter",
                    model,
                    attempts_used,
                    total_elapsed,
                    "exhausted_retries",
                ),
            );
            let error = format!("OpenRouter API failed after 5 attempts: {last_err}");
            finish_llm_guest_span_error(&mut llm_span, "exhausted_retries", &error);
            return Err(error);
        }
    };
    ctx.log(
        "info",
        &format_llm_complete_log(
            "openrouter",
            model,
            attempts_used,
            Context::get_time_millis() - overall_start_ms,
            "success",
        ),
    );

    ctx.log(
        "info",
        &format_gen_ai_usage_log(
            "openrouter",
            model,
            parsed.input_tokens,
            parsed.output_tokens,
            0,
            0,
        ),
    );

    finish_llm_guest_span_success(
        ctx,
        &mut llm_span,
        "openrouter",
        model,
        &parsed.stop_reason,
        parsed.input_tokens,
        parsed.output_tokens,
        0,
        0,
        parsed.response_bytes,
        &parsed.content,
    );

    Ok(parsed.into_llm_response(body_str.len()))
}

/// Extract text and image content from a tool_result content field.
/// Returns (text_output, Vec<(media_type, base64_data)>).
fn extract_text_and_images_from_tool_content(
    content: Option<&Value>,
) -> (String, Vec<(String, String)>) {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();

    match content {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            text_parts.push(t.to_string());
                        }
                    }
                    "image" => {
                        if let Some(source) = block.get("source") {
                            let media_type = source
                                .get("media_type")
                                .and_then(Value::as_str)
                                .unwrap_or("image/png")
                                .to_string();
                            let data = source
                                .get("data")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if !data.is_empty() {
                                images.push((media_type, data));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(Value::String(text)) => {
            text_parts.push(text.clone());
        }
        _ => {}
    }

    (text_parts.join("\n"), images)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenAiResponsesInput {
    input: Vec<Value>,
    tool_calls_as_context: usize,
    tool_outputs_as_context: usize,
}

fn push_openai_user_text_with_images(
    input: &mut Vec<Value>,
    text: &str,
    images: &[(String, String)],
) {
    if images.is_empty() {
        if !text.trim().is_empty() {
            input.push(json!({"role": "user", "content": text}));
        }
        return;
    }

    let mut content = Vec::new();
    if !text.trim().is_empty() {
        content.push(json!({"type": "input_text", "text": text}));
    }
    for (media_type, data) in images {
        content.push(json!({
            "type": "input_image",
            "image_url": format!("data:{media_type};base64,{data}")
        }));
    }
    input.push(json!({
        "type": "message",
        "role": "user",
        "content": content
    }));
}

fn push_openai_assistant_text(input: &mut Vec<Value>, text: &str) {
    if text.trim().is_empty() {
        return;
    }

    input.push(json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}]
    }));
}

fn push_openai_tool_call_context(
    input: &mut Vec<Value>,
    tool_calls_as_context: &mut usize,
    block: &Value,
) {
    *tool_calls_as_context += 1;
    let call_id = block
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or("unknown");
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("unknown_tool");
    let arguments = serde_json::to_string(block.get("input").unwrap_or(&json!({})))
        .unwrap_or_else(|_| "{}".to_string());

    push_openai_assistant_text(input, &format!("Tool call {call_id}: {name}({arguments})"));
}

fn push_openai_tool_result(
    input: &mut Vec<Value>,
    tool_outputs_as_context: &mut usize,
    call_id: &str,
    content: Option<&Value>,
) {
    let (mut output, images) = extract_text_and_images_from_tool_content(content);
    if output.trim().is_empty() {
        if let Some(raw) = content {
            output = stringify_content(raw);
        }
    }

    *tool_outputs_as_context += 1;
    let display_call_id = if call_id.trim().is_empty() {
        "unknown"
    } else {
        call_id
    };
    let fallback_text = if output.trim().is_empty() {
        format!("Tool result for call {display_call_id} was empty.")
    } else {
        format!("Tool result for call {display_call_id}:\n{output}")
    };
    push_openai_user_text_with_images(input, &fallback_text, &images);
}

fn build_openai_responses_input(messages: &[Value]) -> OpenAiResponsesInput {
    let mut input = Vec::<Value>::new();
    let mut tool_calls_as_context = 0usize;
    let mut tool_outputs_as_context = 0usize;

    for msg in messages {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        match role {
            "user" => {
                if let Some(content) = msg.get("content").and_then(Value::as_str) {
                    input.push(json!({"role": "user", "content": content}));
                } else if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                    let mut user_text = Vec::<String>::new();
                    for block in blocks {
                        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                        match block_type {
                            "tool_result" => {
                                let call_id = block
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                push_openai_tool_result(
                                    &mut input,
                                    &mut tool_outputs_as_context,
                                    call_id,
                                    block.get("content"),
                                );
                            }
                            "text" => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    user_text.push(text.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() {
                        input.push(json!({"role": "user", "content": user_text.join("\n")}));
                    }
                }
            }
            "assistant" => {
                if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    push_openai_assistant_text(&mut input, text);
                                }
                            }
                            "tool_use" => {
                                push_openai_tool_call_context(
                                    &mut input,
                                    &mut tool_calls_as_context,
                                    block,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            "tool_result" => {
                let tool_use_id = msg.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
                push_openai_tool_result(
                    &mut input,
                    &mut tool_outputs_as_context,
                    tool_use_id,
                    msg.get("content"),
                );
            }
            _ => {}
        }
    }

    OpenAiResponsesInput {
        input,
        tool_calls_as_context,
        tool_outputs_as_context,
    }
}

/// Call OpenAI Codex Responses API (chatgpt.com/backend-api/codex/responses).
///
/// Uses the Responses API format (not Chat Completions): instructions, input, stream=true.
/// The WASM http_call buffers the full SSE stream — we parse the response.completed event.
fn reasoning_effort_from_options(provider_options_json: &str) -> &'static str {
    let parsed: Option<Value> = serde_json::from_str(provider_options_json).ok();
    match parsed
        .as_ref()
        .and_then(|v| v.get("reasoning_effort"))
        .and_then(Value::as_str)
        .unwrap_or("")
    {
        "minimal" => "minimal",
        "low" => "low",
        "high" => "high",
        _ => "medium",
    }
}

fn call_openai(
    ctx: &Context,
    temper_api_url: &str,
    tenant: &str,
    api_key: &str,
    api_url: &str,
    codex_account_id: Option<&str>,
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    temperature: f64,
    provider: &str,
    provider_options_json: &str,
) -> Result<LlmResponse, String> {
    // Convert Anthropic-format messages to Responses API input format
    let pre_convert_types: Vec<String> = messages
        .iter()
        .map(|m| {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
            let ct = if m.get("content").and_then(Value::as_str).is_some() {
                "str".to_string()
            } else if let Some(arr) = m.get("content").and_then(Value::as_array) {
                let block_types: Vec<&str> = arr
                    .iter()
                    .filter_map(|b| b.get("type").and_then(Value::as_str))
                    .collect();
                format!("arr[{}]", block_types.join(","))
            } else {
                "?".to_string()
            };
            format!("{}:{}", role, ct)
        })
        .collect();
    ctx.log(
        "info",
        &format!(
            "session_turn: openai pre-convert messages={} types={:?}",
            messages.len(),
            pre_convert_types
        ),
    );

    let converted_input = build_openai_responses_input(messages);
    if converted_input.tool_calls_as_context > 0 || converted_input.tool_outputs_as_context > 0 {
        ctx.log(
            "warn",
            &format!(
                "session_turn: openai downgraded {} historical tool call(s) and {} tool output(s) to conversation context",
                converted_input.tool_calls_as_context,
                converted_input.tool_outputs_as_context
            ),
        );
    }
    let input = converted_input.input;

    // Convert tools to Responses API format
    let codex_tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            let name = t.get("name").and_then(Value::as_str).unwrap_or("");
            let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
            let schema = t.get("input_schema").cloned().unwrap_or(json!({}));
            json!({
                "type": "function",
                "name": name,
                "description": desc,
                "parameters": schema,
                "strict": false
            })
        })
        .collect();

    let mut body = json!({
        "model": model,
        "instructions": system_prompt,
        "input": input,
        "stream": true,
        "store": false,
        "reasoning": {
            // Session-configurable via provider_options_json
            // {"reasoning_effort": ...}; "medium" preserves prior behavior.
            "effort": reasoning_effort_from_options(provider_options_json),
            "summary": "auto",
        },
    });
    // Codex API does not support the temperature parameter
    if provider != "openai_codex" {
        body["temperature"] = json!(temperature);
    }
    if !codex_tools.is_empty() {
        body["tools"] = json!(codex_tools);
        // "auto" lets the model choose text or tool calls. "required" forces
        // a tool call every turn, which creates an infinite loop when the model
        // wants to respond with text (e.g., "hello").
        body["tool_choice"] = json!("auto");
    }

    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("JSON serialize error: {e}"))?;

    let mut headers = build_openai_headers(provider, api_key, codex_account_id);
    if !headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("accept"))
    {
        headers.push(("accept".to_string(), "text/event-stream".to_string()));
    }
    let mut llm_span = start_llm_guest_span(
        ctx,
        provider,
        model,
        temperature,
        LLM_MAX_TOKENS,
        system_prompt,
        messages,
        &ctx.entity_id,
    );

    // Log input types for debugging conversation format issues
    let input_types: Vec<String> = input
        .iter()
        .map(|i| {
            let t = i
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| i.get("role").and_then(Value::as_str))
                .unwrap_or("?");
            t.to_string()
        })
        .collect();
    // Log top-level body keys (not full body — system prompt can be huge)
    let body_keys: Vec<&str> = body
        .as_object()
        .map(|m| m.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();
    ctx.log(
        "info",
        &format!(
            "session_turn: calling OpenAI API, model={model}, input={}, types={:?}, url={api_url}, body_keys={body_keys:?}",
            input.len(),
            input_types,
        ),
    );

    let mut last_err = String::new();
    let mut parsed_stream = None;
    let mut live_progress = LlmLiveProgress::new(ctx, temper_api_url, tenant, provider, model);

    for attempt in 0..LLM_MAX_ATTEMPTS {
        let attempt_num = attempt + 1;
        if attempt > 0 {
            ctx.log(
                "warn",
                &format!("session_turn: OpenAI Codex retry {attempt_num}/{LLM_MAX_ATTEMPTS}"),
            );
        }
        let mut accumulator = OpenAiStreamAccumulator::default();
        let stream_result = post_sse_streaming(ctx, api_url, &headers, &body_str, |data| {
            let deltas = accumulator.ingest_data(data)?;
            live_progress.emit_deltas(&deltas);
            Ok(())
        });

        match stream_result {
            Ok(r) if r.status >= 200 && r.status < 300 => {
                match accumulator.finalize(r.response_bytes) {
                    Ok(parsed) => {
                        parsed_stream = Some(parsed);
                        break;
                    }
                    Err(err) => {
                        last_err = err.to_string();
                        let visible =
                            err.semantic_output_seen || live_progress.saw_semantic_output();
                        if should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                            ctx.log(
                                "warn",
                                &format!("session_turn: OpenAI Codex stream parse failed before visible output, will retry: {last_err}"),
                            );
                            continue;
                        }
                        let error = format!(
                            "OpenAI Codex stream failed after visible output or final attempt: {last_err}"
                        );
                        finish_llm_guest_span_error(&mut llm_span, "stream_parse_error", &error);
                        return Err(error);
                    }
                }
            }
            Ok(r) if r.status == 429 => {
                last_err = format!("OpenAI Codex API rate limited (429)");
                if live_progress.saw_semantic_output() {
                    let error = format!(
                        "OpenAI Codex stream was rate limited after visible output: {}",
                        &r.body[..r.body.len().min(300)]
                    );
                    finish_llm_guest_span_error(&mut llm_span, "rate_limited", &error);
                    return Err(error);
                }
                continue;
            }
            Ok(r) => {
                let snippet = &r.body[..r.body.len().min(300)];
                if provider == "openai_codex"
                    && is_openai_codex_token_expired_error(r.status as u16, &r.body)
                {
                    ctx.log(
                        "warn",
                        "session_turn: OpenAI Codex token expired; dispatching auth refresh gate",
                    );
                    let error = provider_auth_expired_error(&r.body);
                    finish_llm_guest_span_error(&mut llm_span, "provider_auth_expired", &error);
                    return Err(error);
                }
                ctx.log(
                    "error",
                    &format!(
                        "session_turn: OpenAI Codex API error status={} body={snippet}",
                        r.status
                    ),
                );
                let error = format!("OpenAI Codex API returned {}: {snippet}", r.status);
                finish_llm_guest_span_error(&mut llm_span, "http_error", &error);
                return Err(error);
            }
            Err(e) => {
                last_err = e.to_string();
                ctx.log(
                    "warn",
                    &format_openai_codex_host_http_failure_log(
                        attempt_num,
                        LLM_MAX_ATTEMPTS,
                        &last_err,
                    ),
                );
                let visible = e.semantic_output_seen || live_progress.saw_semantic_output();
                if !should_retry_stream_failure(attempt_num, LLM_MAX_ATTEMPTS, visible) {
                    let error = format!(
                        "OpenAI Codex stream failed after visible output or final attempt: {last_err}"
                    );
                    finish_llm_guest_span_error(&mut llm_span, "stream_error", &error);
                    return Err(error);
                }
                continue;
            }
        }
    }

    let parsed = match parsed_stream {
        Some(parsed) => parsed,
        None => {
            let error = format_openai_codex_exhausted_error(LLM_MAX_ATTEMPTS, &last_err);
            finish_llm_guest_span_error(&mut llm_span, "exhausted_retries", &error);
            return Err(error);
        }
    };
    let content_blocks = parsed.content.as_array().map(Vec::len).unwrap_or(0);

    ctx.log(
        "info",
        &format!(
            "session_turn: OpenAI Codex response: blocks={}, stop={}, in={}, out={}",
            content_blocks, parsed.stop_reason, parsed.input_tokens, parsed.output_tokens
        ),
    );

    finish_llm_guest_span_success(
        ctx,
        &mut llm_span,
        provider,
        model,
        &parsed.stop_reason,
        parsed.input_tokens,
        parsed.output_tokens,
        0,
        0,
        parsed.response_bytes,
        &parsed.content,
    );

    Ok(parsed.into_llm_response(body_str.len()))
}

fn stringify_content(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        value.to_string()
    }
}

#[allow(dead_code)]
fn emit_progress_ignore(ctx: &Context, payload: Value) {
    let _ = (ctx, payload);
}

fn agent_headers(
    ctx: &Context,
    tenant: &str,
    content_type: Option<&str>,
    accept: Option<&str>,
) -> Vec<(String, String)> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    runtime_headers(ctx, tenant, &fields, content_type, accept)
}

fn send_heartbeat(ctx: &Context, temper_api_url: &str, tenant: &str) -> Result<(), String> {
    let url = format!(
        "{temper_api_url}/tdata/Sessions('{}')/TemperPaw.Heartbeat",
        ctx.entity_id
    );
    let body = json!({ "last_heartbeat_at": timestamp_millis_string() });
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let headers = runtime_headers_as(
        ctx,
        tenant,
        &fields,
        "system",
        Some("application/json"),
        None,
    );
    let _ = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    Ok(())
}

fn send_progress(ctx: &Context, temper_api_url: &str, tenant: &str) -> Result<(), String> {
    let url = format!(
        "{temper_api_url}/tdata/Sessions('{}')/TemperPaw.ProgressMade",
        ctx.entity_id
    );
    let body = json!({ "last_progress_at": timestamp_millis_string() });
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let headers = runtime_headers_as(
        ctx,
        tenant,
        &fields,
        "system",
        Some("application/json"),
        None,
    );
    let _ = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    Ok(())
}

fn config_bool(ctx: &Context, primary_key: &str, fallback_key: Option<&str>) -> bool {
    ctx.config
        .get(primary_key)
        .or_else(|| fallback_key.and_then(|key| ctx.config.get(key)))
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn provider_progress_dispatch_enabled(ctx: &Context) -> bool {
    config_bool(
        ctx,
        "provider_progress_dispatch_enabled",
        Some("session_provider_progress_enabled"),
    )
}

fn provider_initial_heartbeat_enabled(ctx: &Context) -> bool {
    config_bool(
        ctx,
        "provider_initial_heartbeat_enabled",
        Some("session_provider_initial_heartbeat_enabled"),
    )
}

fn should_send_initial_provider_heartbeat(
    mock_hang: bool,
    initial_heartbeat_enabled: bool,
) -> bool {
    initial_heartbeat_enabled && !mock_hang
}

fn mock_plan_requests_hang(messages: &[Value]) -> bool {
    if let Some(steps) = extract_mock_plan(messages)
        && steps
            .iter()
            .any(|step| step.get("mode").and_then(Value::as_str) == Some("hang"))
    {
        return true;
    }
    latest_user_text(messages)
        .map(|text| text.contains("[mock-hang]"))
        .unwrap_or(false)
}

fn simulate_mock_hang(ctx: &Context) -> Result<(), String> {
    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let base_url = resolve_temper_api_url(ctx, &fields);
    let url = format!(
        "{base_url}/observe/entities/{}/{}/wait?statuses=__never__&timeout_ms=10000&poll_ms=250",
        ctx.entity_type, ctx.entity_id
    );
    let headers = agent_headers(ctx, &ctx.tenant, None, Some("application/json"));
    let _ = ctx.http_call("GET", &url, &headers, "")?;
    Ok(())
}

fn extract_mock_plan(messages: &[Value]) -> Option<Vec<Value>> {
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let raw = stringify_content(message.get("content").unwrap_or(&Value::Null));
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(steps) = parsed.get("steps").and_then(Value::as_array) {
            return Some(steps.clone());
        }
        if let Some(steps) = parsed
            .get("mock_plan")
            .and_then(|value| value.get("steps"))
            .and_then(Value::as_array)
        {
            return Some(steps.clone());
        }
    }
    None
}

fn build_mock_step_response(
    messages: &[Value],
    assembled_system_prompt: &str,
    assistant_turns: usize,
    step: &Value,
) -> Result<LlmResponse, String> {
    if step.get("mode").and_then(Value::as_str) == Some("hang") {
        return Ok(mock_text_response(
            messages,
            "mock hang placeholder".to_string(),
        ));
    }

    let mut content = Vec::<Value>::new();
    if let Some(text) = step.get("text").and_then(Value::as_str) {
        let resolved = resolve_mock_template(
            text,
            assembled_system_prompt,
            latest_user_text(messages).as_deref().unwrap_or(""),
        );
        if !resolved.is_empty() {
            content.push(json!({ "type": "text", "text": resolved }));
        }
    }

    if let Some(tool_calls) = step.get("tool_calls").and_then(Value::as_array) {
        for (index, tool_call) in tool_calls.iter().enumerate() {
            let name = tool_call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown_tool");
            let input = tool_call.get("input").cloned().unwrap_or_else(|| json!({}));
            let id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("mock-tool-{assistant_turns}-{index}"));
            content.push(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }));
        }
    }

    if content
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
    {
        let serialized_content = serde_json::to_string(&content).unwrap_or_default();
        let output_len = serialized_content.len() as i64;
        return Ok(LlmResponse {
            content: Value::Array(content),
            stop_reason: "tool_use".to_string(),
            input_tokens: estimate_message_tokens(messages),
            output_tokens: output_len,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 0,
            response_bytes: serialized_content.len(),
            token_signals: None,
        });
    }

    let final_text = step
        .get("final_text")
        .or_else(|| step.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("mock provider completed");
    Ok(mock_text_response(
        messages,
        resolve_mock_template(
            final_text,
            assembled_system_prompt,
            latest_user_text(messages).as_deref().unwrap_or(""),
        ),
    ))
}

fn mock_text_response(messages: &[Value], text: String) -> LlmResponse {
    LlmResponse {
        content: json!([{ "type": "text", "text": text.clone() }]),
        stop_reason: "end_turn".to_string(),
        input_tokens: estimate_message_tokens(messages),
        output_tokens: text.len() as i64,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        request_bytes: 0,
        response_bytes: text.len(),
        token_signals: None,
    }
}

fn estimate_message_tokens(messages: &[Value]) -> i64 {
    messages
        .iter()
        .map(|message| {
            message
                .get("content")
                .map(stringify_content)
                .unwrap_or_default()
                .len() as i64
        })
        .sum::<i64>()
}

fn latest_user_text(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| stringify_content(message.get("content").unwrap_or(&Value::Null)))
}

fn resolve_mock_template(
    template: &str,
    assembled_system_prompt: &str,
    latest_user: &str,
) -> String {
    let mut text = template.to_string();
    text = text.replace("{{latest_user}}", latest_user);
    text = text.replace(
        "{{memory_block}}",
        &extract_tag_block(assembled_system_prompt, "agent_memory"),
    );
    text = text.replace(
        "{{memory_keys}}",
        &extract_memory_keys(assembled_system_prompt).join(", "),
    );
    text = text.replace(
        "{{memory_count}}",
        &extract_memory_keys(assembled_system_prompt)
            .len()
            .to_string(),
    );
    text = text.replace(
        "{{skills_block}}",
        &extract_tag_block(assembled_system_prompt, "available_skills"),
    );
    text
}

fn extract_tag_block(text: &str, tag: &str) -> String {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let Some(start) = text.find(&start_tag) else {
        return String::new();
    };
    let Some(end) = text[start..].find(&end_tag) else {
        return String::new();
    };
    text[start..start + end + end_tag.len()].to_string()
}

fn extract_memory_keys(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let marker = "key=\"";
            let start = line.find(marker)? + marker.len();
            let rest = &line[start..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// Unreachable, and kept in step with the live `convert_messages_to_chat`
/// anyway: the fallback tool-call id is scoped per message, because a reader
/// who mistakes this for the live path must not find the unscoped rule here.
fn convert_messages_to_openrouter(messages: &[Value]) -> Vec<Value> {
    let mut out = Vec::<Value>::new();
    for (message_index, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = msg.get("content").cloned().unwrap_or(json!(""));

        match content {
            Value::String(text) => {
                out.push(json!({
                    "role": role,
                    "content": text,
                }));
            }
            Value::Array(blocks) => {
                if role == "assistant" {
                    let mut text_chunks = Vec::<String>::new();
                    let mut tool_calls = Vec::<Value>::new();
                    for (idx, block) in blocks.iter().enumerate() {
                        match block.get("type").and_then(Value::as_str).unwrap_or("") {
                            "text" => {
                                if let Some(t) = block.get("text").and_then(Value::as_str) {
                                    text_chunks.push(t.to_string());
                                }
                            }
                            "tool_use" => {
                                let id = block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| {
                                        // Position within the message is not
                                        // unique across a conversation; scope
                                        // it so two id-less assistant turns
                                        // cannot send the provider the same
                                        // call id, which would collapse two
                                        // decisions into one (ADR-0035 §14).
                                        synthetic_tool_call_id(
                                            &format!("msg{message_index}"),
                                            "tool",
                                            idx + 1,
                                        )
                                    });
                                let name = block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown_tool");
                                let input = block.get("input").cloned().unwrap_or(json!({}));
                                tool_calls.push(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": input.to_string(),
                                    }
                                }));
                            }
                            _ => {}
                        }
                    }

                    let mut assistant = json!({
                        "role": "assistant",
                        "content": text_chunks.join("\n"),
                    });
                    if !tool_calls.is_empty() {
                        assistant["tool_calls"] = json!(tool_calls);
                    }
                    out.push(assistant);
                } else if role == "user" {
                    let mut user_text = Vec::<String>::new();
                    for block in &blocks {
                        match block.get("type").and_then(Value::as_str).unwrap_or("") {
                            "tool_result" => {
                                let tool_call_id = block
                                    .get("tool_use_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown_tool_call");
                                let (text_output, images) =
                                    extract_text_and_images_from_tool_content(block.get("content"));
                                let content = if text_output.is_empty() {
                                    stringify_content(
                                        block
                                            .get("content")
                                            .unwrap_or(&Value::String(String::new())),
                                    )
                                } else {
                                    text_output
                                };
                                out.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_call_id,
                                    "content": content,
                                }));
                                // Inject user message with images for visual context
                                for (media_type, data) in &images {
                                    out.push(json!({
                                        "role": "user",
                                        "content": [{
                                            "type": "image_url",
                                            "image_url": {
                                                "url": format!("data:{media_type};base64,{data}")
                                            }
                                        }]
                                    }));
                                }
                            }
                            "text" => {
                                if let Some(t) = block.get("text").and_then(Value::as_str) {
                                    user_text.push(t.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() {
                        out.push(json!({
                            "role": "user",
                            "content": user_text.join("\n"),
                        }));
                    }
                } else {
                    out.push(json!({
                        "role": role,
                        "content": Value::Array(blocks),
                    }));
                }
            }
            other => {
                out.push(json!({
                    "role": role,
                    "content": other,
                }));
            }
        }
    }
    out
}

fn convert_tools_to_openrouter(tools: &[Value]) -> Vec<Value> {
    let mut out = Vec::<Value>::new();
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let parameters = tool
            .get("input_schema")
            .cloned()
            .unwrap_or(json!({"type": "object", "properties": {}}));
        out.push(json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": parameters,
            }
        }));
    }
    out
}

/// Build tool definitions for the LLM.
///
/// Returns a single `execute` tool for the Monty REPL. Agents write Python
/// code using `temper.*` and `sandbox.*` objects. The method listing is
/// inline in the tool description so agents see it immediately.
pub fn run_provider_caller() -> Result<(), String> {
    let started_at = Context::get_time_millis();
    let ctx = Context::from_host()?;
    ctx.log("info", "provider_caller: starting");

    let fields = ctx
        .entity_state
        .get("fields")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let prepared_context_file_id = fields
        .get("prepared_context_file_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let prepared_context_inline_json =
        read_state_string_field(&ctx, &fields, "prepared_context_inline_json");
    if prepared_context_file_id.is_empty() && prepared_context_inline_json.is_empty() {
        return Err(
            "provider_caller: missing prepared_context_inline_json or prepared_context_file_id"
                .to_string(),
        );
    }
    let temper_api_url = resolve_temper_api_url(&ctx, &fields);
    let tenant = &ctx.tenant;
    let provider_caller_budget_ms = configured_budget_ms(
        &ctx,
        &fields,
        "provider_caller_budget_ms",
        DEFAULT_PROVIDER_CALLER_BUDGET_MS,
    );
    let read_started_at = Context::get_time_millis();
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
        "provider_caller",
        "read_prepared_artifact",
        read_started_at,
        if prepared_result.is_ok() {
            "ok"
        } else {
            "error"
        },
    );
    let prepared = prepared_result?;
    check_phase_budget(
        &ctx,
        "provider_caller",
        started_at,
        provider_caller_budget_ms,
        "read_prepared_artifact",
    )?;
    let provider_raw = fields
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let model_raw = fields.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let temperature: f64 = fields
        .get("temperature")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.0);
    let provider_options_json = read_state_string_field(&ctx, &fields, "provider_options_json");
    let (provider, model, api_key) = resolve_provider_and_model(&ctx, provider_raw, model_raw)?;

    let anthropic_api_url = ctx
        .config
        .get("anthropic_api_url")
        .cloned()
        .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
    let openai_compatible_api_url = if matches!(
        provider.as_str(),
        "openrouter"
            | "huggingface"
            | "fireworks"
            | "sakana_fugu"
            | "local_openai"
            | "openai_compatible"
    ) {
        configured_openai_compatible_api_url(&ctx, &provider)?
    } else {
        String::new()
    };
    let openai_compatible_extra_headers = configured_openai_compatible_headers(&ctx, &provider)?;
    let openai_api_url = select_openai_responses_url(&ctx.config, &provider);
    let openai_codex_account_id = if provider == "openai_codex" {
        ctx.config
            .get("openai_codex_account_id")
            .filter(|value| !value.trim().is_empty() && !is_unresolved_secret_template(value))
            .cloned()
            .or_else(|| extract_chatgpt_account_id_from_jwt(&api_key))
    } else {
        None
    };
    if provider == "openai_codex" && openai_codex_account_id.is_none() {
        return Err(
            "openai_codex requires openai_codex_account_id or a ChatGPT OAuth token containing chatgpt_account_id"
                .to_string(),
        );
    }
    let anthropic_auth_mode = ctx
        .config
        .get("anthropic_auth_mode")
        .cloned()
        .unwrap_or_else(|| "auto".to_string());
    let openrouter_site_url = ctx
        .config
        .get("openrouter_site_url")
        .cloned()
        .unwrap_or_default();
    let openrouter_app_name = ctx
        .config
        .get("openrouter_app_name")
        .cloned()
        .unwrap_or_else(|| "temperpaw-agent".to_string());

    let mock_hang = provider == "mock" && mock_plan_requests_hang(&prepared.messages);
    if should_send_initial_provider_heartbeat(mock_hang, provider_initial_heartbeat_enabled(&ctx)) {
        let _ = send_heartbeat(&ctx, &temper_api_url, tenant);
    }
    let typing_agent_id = fields
        .get("agent_id")
        .or_else(|| fields.get("AgentId"))
        .and_then(|v| v.as_str())
        .unwrap_or(&ctx.entity_id);
    if should_send_provider_typing_indicator(&ctx.entity_id, &fields) {
        send_typing_indicator(&ctx, &temper_api_url, tenant, typing_agent_id);
    } else {
        ctx.log(
            "debug",
            "provider_caller: skipping typing indicator for direct or inline route",
        );
    }

    let provider_call_started_at = Context::get_time_millis();
    let provider_progress_enabled = provider_progress_dispatch_enabled(&ctx);
    let response_result = run_with_provider_progress(
        |boundary| {
            ctx.log(
                "debug",
                &format!(
                    "provider_caller: provider progress boundary={boundary:?} provider={provider} model={model}"
                ),
            );
            if provider_progress_enabled {
                let _ = send_progress(&ctx, &temper_api_url, tenant);
            }
        },
        || match provider.as_str() {
            "mock" => call_mock(
                &ctx,
                &prepared.messages,
                &prepared.system_prompt,
                &prepared.tools,
            ),
            "anthropic" => call_anthropic(
                &ctx,
                &temper_api_url,
                tenant,
                &api_key,
                &anthropic_api_url,
                &model,
                &prepared.system_prompt,
                &prepared.messages,
                &prepared.tools,
                &anthropic_auth_mode,
                temperature,
            ),
            "openrouter" => call_openai_compatible_chat(
                &ctx,
                &temper_api_url,
                tenant,
                &provider,
                &api_key,
                &openai_compatible_api_url,
                &model,
                &prepared.system_prompt,
                &prepared.messages,
                &prepared.tools,
                &openrouter_site_url,
                &openrouter_app_name,
                &openai_compatible_extra_headers,
                temperature,
                &provider_options_json,
            ),
            "huggingface" | "fireworks" | "sakana_fugu" | "local_openai" | "openai_compatible" => {
                call_openai_compatible_chat(
                    &ctx,
                    &temper_api_url,
                    tenant,
                    &provider,
                    &api_key,
                    &openai_compatible_api_url,
                    &model,
                    &prepared.system_prompt,
                    &prepared.messages,
                    &prepared.tools,
                    "",
                    "",
                    &openai_compatible_extra_headers,
                    temperature,
                    &provider_options_json,
                )
            }
            "openai" | "openai_codex" => call_openai(
                &ctx,
                &temper_api_url,
                tenant,
                &api_key,
                &openai_api_url,
                openai_codex_account_id.as_deref(),
                &model,
                &prepared.system_prompt,
                &prepared.messages,
                &prepared.tools,
                temperature,
                &provider,
                &provider_options_json,
            ),
            other => Err(format!("unsupported LLM provider: {other}")),
        },
    );
    emit_phase_step_duration(
        &ctx,
        "provider_caller",
        "provider_http",
        provider_call_started_at,
        if response_result.is_ok() {
            "ok"
        } else {
            "error"
        },
    );
    if let Err(err) = &response_result
        && let Some(reason) = provider_auth_expired_reason(err)
    {
        set_success_result(
            "ProviderAuthExpired",
            &json!({
                "provider_auth_error": reason,
            }),
        );
        emit_phase_total_duration(&ctx, "provider_caller", started_at, "provider_auth_expired");
        return Ok(());
    }
    let response = response_result?;
    check_phase_budget(
        &ctx,
        "provider_caller",
        started_at,
        provider_caller_budget_ms,
        "provider_http",
    )?;

    let metric_tags = session_metric_tags(&provider, &model);
    emit_metric_ignore(
        &ctx,
        "temper_session_provider_request_bytes",
        response.request_bytes as f64,
        &metric_tags,
        Some("gauge"),
    );
    emit_metric_ignore(
        &ctx,
        "temper_session_provider_response_bytes",
        response.response_bytes as f64,
        &metric_tags,
        Some("gauge"),
    );

    let artifact = ProviderResponseArtifact {
        version: 1,
        provider: provider.clone(),
        model: model.clone(),
        content: response.content,
        stop_reason: response.stop_reason,
        input_tokens: response.input_tokens,
        output_tokens: response.output_tokens,
        cache_read_input_tokens: response.cache_read_input_tokens,
        cache_creation_input_tokens: response.cache_creation_input_tokens,
        request_bytes: response.request_bytes,
        response_bytes: response.response_bytes,
        token_signals: response.token_signals,
    };
    let artifact_json = serde_json::to_string(&artifact)
        .map_err(|e| format!("provider response artifact serialize: {e}"))?;
    let stage_started_at = Context::get_time_millis();
    emit_phase_step_duration(
        &ctx,
        "provider_caller",
        "write_provider_response_artifact",
        stage_started_at,
        "ok",
    );
    check_phase_budget(
        &ctx,
        "provider_caller",
        started_at,
        provider_caller_budget_ms,
        "write_provider_response_artifact",
    )?;

    let params =
        build_provider_response_ready_params_with_inline("", &artifact_json, &prepared, &artifact);
    set_success_result("ProviderResponseReady", &params);
    emit_phase_total_duration(
        &ctx,
        "provider_caller",
        started_at,
        "provider_response_ready",
    );
    Ok(())
}

fn resolve_provider_and_model(
    ctx: &Context,
    provider_raw: &str,
    model_raw: &str,
) -> Result<(String, String, String), String> {
    if model_raw.trim().is_empty() {
        return Err(
            "Session model is required; configure the Agent or pass an explicit override"
                .to_string(),
        );
    }
    if provider_raw.trim().is_empty() {
        return Err(
            "Session provider is required; configure the Agent or pass an explicit override"
                .to_string(),
        );
    }
    let provider = normalize_provider(provider_raw);
    let api_key = if provider == "mock" {
        String::new()
    } else {
        let key = resolve_provider_api_key(ctx, &provider)?;
        if is_unresolved_secret_template(&key) {
            return Err(format!(
                "provider={provider} api key is unresolved secret template: '{key}'. set tenant secret and retry"
            ));
        } else {
            key
        }
    };

    let model = model_raw.trim().to_string();

    if !provider_allows_empty_api_key(&provider) && api_key.is_empty() {
        return Err(format!("missing API key for provider={provider}"));
    }

    Ok((provider, model, api_key))
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

fn field_str<'a>(fields: &'a Value, names: &[&str]) -> &'a str {
    names
        .iter()
        .find_map(|name| fields.get(*name)?.as_str())
        .unwrap_or("")
}

fn should_send_provider_typing_indicator(entity_id: &str, fields: &Value) -> bool {
    let parent_session_id = field_str(fields, &["parent_session_id", "ParentSessionId"]).trim();
    if !parent_session_id.is_empty() && parent_session_id != entity_id {
        return true;
    }

    let reply_channel_id = field_str(fields, &["reply_channel_id", "ReplyChannelId"]).trim();
    let reply_thread_id = field_str(fields, &["reply_thread_id", "ReplyThreadId"]).trim();
    if reply_channel_id.is_empty() || reply_thread_id.is_empty() {
        return false;
    }

    let reply_channel_type = field_str(fields, &["reply_channel_type", "ReplyChannelType"]).trim();
    let reply_channel_type = reply_channel_type.to_ascii_lowercase();
    !matches!(reply_channel_type.as_str(), "cli" | "tui")
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

#[cfg(test)]
mod tests {
    #[test]
    fn reasoning_effort_defaults_to_medium_and_accepts_valid_levels() {
        use super::reasoning_effort_from_options as effort;
        assert_eq!(effort(""), "medium");
        assert_eq!(effort("not json"), "medium");
        assert_eq!(effort("{}"), "medium");
        assert_eq!(effort(r#"{"reasoning_effort":"high"}"#), "high");
        assert_eq!(effort(r#"{"reasoning_effort":"low"}"#), "low");
        assert_eq!(effort(r#"{"reasoning_effort":"minimal"}"#), "minimal");
        assert_eq!(effort(r#"{"reasoning_effort":"xhigh"}"#), "medium");
    }

    use super::*;

    /// The dead OpenRouter conversion is kept in step with the live one. This
    /// exact line regressed once already — a rebase restored a copy predating
    /// the fix — so the invariant is pinned here rather than trusted to the
    /// code being unreachable today. Position within a message is not unique
    /// across a conversation: two id-less assistant turns would otherwise send
    /// the provider the same call id, and the emitter would collapse two
    /// decisions into one (ADR-0035 section 14).
    #[test]
    fn openrouter_conversion_scopes_missing_tool_call_ids_per_message() {
        let messages = vec![
            json!({"role":"assistant","content":[{"type":"tool_use","name":"a","input":{}}]}),
            json!({"role":"assistant","content":[{"type":"tool_use","name":"b","input":{}}]}),
        ];
        let converted = convert_messages_to_openrouter(&messages);
        let ids: Vec<&str> = converted
            .iter()
            .filter_map(|message| message.get("tool_calls"))
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|call| call["id"].as_str())
            .collect();

        assert_eq!(ids.len(), 2);
        assert_ne!(
            ids[0], ids[1],
            "two id-less assistant turns must not share a synthetic call id"
        );
    }

    /// A server that carries the same signals at both levels of one event must
    /// not have them stored twice. Completion-side signals accumulate across
    /// events, and with a single signal present there is no second array to
    /// disagree on length — so the doubling would reach an RL consumer as real
    /// token ids. Same class as the chat accumulator's; these two are the other
    /// two wire shapes.
    #[test]
    fn openrouter_event_contributes_each_token_signal_once() {
        let mut accumulator = OpenRouterStreamAccumulator::default();
        accumulator
            .ingest_data(
                &json!({
                    "usage": {"completion_tokens": 2, "completion_token_ids": [7, 8]},
                    "choices": [{"completion_token_ids": [7, 8], "delta": {"content": "hi"}}],
                })
                .to_string(),
            )
            .expect("event parses");

        let signals = accumulator
            .token_signals
            .clone()
            .expect("token signals recorded");
        assert_eq!(
            signals["completion_token_ids"],
            json!([7, 8]),
            "the repeated payload must be taken once, not concatenated"
        );
    }

    #[test]
    fn openai_response_completed_contributes_each_token_signal_once() {
        let mut accumulator = OpenAiStreamAccumulator::default();
        accumulator
            .ingest_data(
                &json!({
                    "type": "response.completed",
                    "response": {
                        "completion_token_ids": [4, 5, 6],
                        "usage": {
                            "input_tokens": 3,
                            "output_tokens": 3,
                            "completion_token_ids": [4, 5, 6],
                        },
                    },
                })
                .to_string(),
            )
            .expect("event parses");

        let signals = accumulator
            .token_signals
            .clone()
            .expect("token signals recorded");
        assert_eq!(
            signals["completion_token_ids"],
            json!([4, 5, 6]),
            "response and response.usage are one event, not two measurements"
        );
        assert_eq!(
            accumulator.usage["output_tokens"], 3,
            "usage accounting is still captured"
        );
    }

    #[test]
    fn provider_progress_wrapper_emits_start_and_end_on_success() {
        let mut events = Vec::new();

        let result = run_with_provider_progress(|event| events.push(event), || Ok::<_, String>(42));

        assert_eq!(result, Ok(42));
        assert_eq!(
            events,
            vec![
                ProviderProgressBoundary::Start,
                ProviderProgressBoundary::End
            ]
        );
    }

    #[test]
    fn provider_progress_wrapper_emits_end_on_error() {
        let mut events = Vec::new();

        let result = run_with_provider_progress(
            |event| events.push(event),
            || Err::<(), _>("provider failed".to_string()),
        );

        assert_eq!(result, Err("provider failed".to_string()));
        assert_eq!(
            events,
            vec![
                ProviderProgressBoundary::Start,
                ProviderProgressBoundary::End
            ]
        );
    }

    #[test]
    fn initial_provider_heartbeat_is_opt_in_and_never_masks_mock_hang() {
        assert!(
            !should_send_initial_provider_heartbeat(false, false),
            "fast provider path should not emit an eager Session.Heartbeat by default"
        );
        assert!(
            should_send_initial_provider_heartbeat(false, true),
            "operator opt-in should preserve the old eager heartbeat behavior"
        );
        assert!(
            !should_send_initial_provider_heartbeat(true, true),
            "mock hang proof path should still exercise timeout behavior without a pre-heartbeat"
        );
    }

    #[test]
    fn typing_indicator_is_route_aware() {
        assert!(
            !should_send_provider_typing_indicator("ss-direct", &json!({})),
            "direct no-route sessions should not spend provider time on ChannelSession lookup"
        );
        assert!(
            !should_send_provider_typing_indicator(
                "ss-cli",
                &json!({
                    "reply_channel_id": "cli-channel",
                    "reply_thread_id": "thread-1",
                    "reply_channel_type": "cli"
                })
            ),
            "inline CLI routes have no external typing indicator"
        );
        assert!(
            !should_send_provider_typing_indicator(
                "ss-cli-pascal",
                &json!({
                    "ReplyChannelId": "cli-channel",
                    "ReplyThreadId": "thread-1",
                    "ReplyChannelType": "CLI"
                })
            ),
            "inline route detection should support projected field casing"
        );
        assert!(
            !should_send_provider_typing_indicator(
                "ss-tui",
                &json!({
                    "reply_channel_id": "tui-channel",
                    "reply_thread_id": "thread-1",
                    "reply_channel_type": "tui"
                })
            ),
            "inline TUI routes have no external typing indicator"
        );
        assert!(
            should_send_provider_typing_indicator(
                "ss-discord",
                &json!({
                    "reply_channel_id": "discord-channel",
                    "reply_thread_id": "thread-1",
                    "reply_channel_type": "discord"
                })
            ),
            "explicit Discord routes should preserve typing behavior"
        );
        assert!(
            should_send_provider_typing_indicator(
                "ss-legacy",
                &json!({
                    "reply_channel_id": "legacy-channel",
                    "reply_thread_id": "thread-1"
                })
            ),
            "legacy channel routes without a type may still be webhook-backed"
        );
        assert!(
            should_send_provider_typing_indicator(
                "ss-child",
                &json!({
                    "parent_session_id": "ss-parent"
                })
            ),
            "parented sessions keep inherited channel typing compatibility"
        );
    }

    #[test]
    fn llm_attempt_start_log_includes_provider_model_attempt_and_elapsed() {
        let msg = format_llm_attempt_start_log("anthropic", "claude-sonnet-4.6", 2, 5, 1234);
        assert!(msg.contains("anthropic"));
        assert!(msg.contains("attempt 2/5"));
        assert!(msg.contains("total_elapsed_ms=1234"));
        assert!(msg.contains("model=claude-sonnet-4.6"));
        assert!(msg.contains("start"));
    }

    #[test]
    fn llm_attempt_end_log_includes_http_status_and_body_len() {
        let msg = format_llm_attempt_end_log("openrouter", 1, 850, 200, 4096);
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("attempt 1"));
        assert!(msg.contains("elapsed_ms=850"));
        assert!(msg.contains("http_status=200"));
        assert!(msg.contains("body_len=4096"));
        assert!(msg.contains("end"));
    }

    #[test]
    fn llm_complete_log_summarises_total_and_outcome() {
        let msg = format_llm_complete_log("anthropic", "claude-sonnet-4.6", 3, 9500, "success");
        assert!(msg.contains("complete"));
        assert!(msg.contains("attempts=3"));
        assert!(msg.contains("total_elapsed_ms=9500"));
        assert!(msg.contains("outcome=success"));
    }

    #[test]
    fn hang_hint_triggers_at_sixty_seconds() {
        // Below threshold — no hint.
        assert!(!should_emit_hang_hint(59_999));
        // At threshold — hint fires.
        assert!(should_emit_hang_hint(60_000));
        // Well over threshold — hint fires.
        assert!(should_emit_hang_hint(180_000));
    }

    #[test]
    fn hang_hint_message_mentions_elapsed_and_provider() {
        let msg = format_llm_hang_hint("anthropic", 1, 70_123);
        assert!(msg.to_lowercase().contains("hang"));
        assert!(msg.contains("anthropic"));
        assert!(msg.contains("attempt 1"));
        assert!(msg.contains("70123"));
    }

    #[test]
    fn provider_auth_expired_result_is_action_routable() {
        let body = r#"{"error":{"code":"token_expired"}}"#;
        let err = provider_auth_expired_error(body);

        assert_eq!(provider_auth_expired_reason(&err), Some(body));
        assert_eq!(
            provider_auth_expired_reason("regular provider failure"),
            None
        );
    }

    #[test]
    fn gen_ai_prompt_attr_includes_system_and_messages() {
        let msgs = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        let out = format_gen_ai_prompt_attr("you are helpful", &msgs);
        let v: Value = serde_json::from_str(&out).expect("must be valid JSON");
        assert_eq!(v["system"], "you are helpful");
        assert_eq!(v["messages"].as_array().unwrap().len(), 2);
        assert_eq!(v["messages"][0]["content"], "hello");
    }

    #[test]
    fn gen_ai_prompt_attr_omits_system_when_empty() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let out = format_gen_ai_prompt_attr("", &msgs);
        let v: Value = serde_json::from_str(&out).expect("must be valid JSON");
        assert!(v.get("system").is_none());
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn gen_ai_prompt_attr_truncates_on_utf8_boundary_with_suffix() {
        // Build a huge message payload to force truncation.
        let big = "🎉".repeat(10 * 1024); // 40 KB of 4-byte chars.
        let msgs = vec![json!({"role": "user", "content": big})];
        let out = format_gen_ai_prompt_attr("", &msgs);
        assert!(out.ends_with("…[truncated]"));
        assert!(
            out.len() <= LLM_PROMPT_ATTR_MAX_BYTES + "…[truncated]".len(),
            "attr length {} exceeded expected cap",
            out.len()
        );
        // Truncation must keep the prefix valid UTF-8.
        let _ = std::str::from_utf8(out.as_bytes()).expect("must remain valid utf-8");
    }

    #[test]
    fn gen_ai_usage_log_emits_semconv_keys() {
        let msg = format_gen_ai_usage_log("anthropic", "claude-sonnet-4.6", 120, 480, 40, 80);
        // Required gen_ai semconv attributes are visible as key=value pairs
        // so DD's grok ingestion tags them on the log event.
        assert!(msg.contains("gen_ai.system=anthropic"));
        assert!(msg.contains("gen_ai.request.model=claude-sonnet-4.6"));
        assert!(msg.contains("gen_ai.usage.input_tokens=120"));
        assert!(msg.contains("gen_ai.usage.output_tokens=480"));
        assert!(msg.contains("gen_ai.usage.cache_read_input_tokens=40"));
        assert!(msg.contains("gen_ai.usage.cache_creation_input_tokens=80"));
    }

    #[test]
    fn gen_ai_usage_log_includes_human_prefix() {
        let msg = format_gen_ai_usage_log("openrouter", "anthropic/claude-sonnet-4.6", 0, 0, 0, 0);
        assert!(msg.starts_with("session_turn: usage "));
    }

    #[test]
    fn llm_success_span_attrs_include_datadog_output_messages() {
        let attrs = llm_success_span_attributes(
            "anthropic",
            "claude-sonnet-4.6",
            "end_turn",
            120,
            480,
            40,
            80,
            2048,
            &json!([
                {"type": "text", "text": "The repair is complete."}
            ]),
        );

        let output_messages = attrs["gen_ai.output.messages"]
            .as_str()
            .expect("gen_ai.output.messages should be a serialized JSON attribute");
        let parsed: Value =
            serde_json::from_str(output_messages).expect("output messages must be JSON");
        assert_eq!(parsed[0]["role"], "assistant");
        assert_eq!(parsed[0]["finish_reason"], "end_turn");
        assert_eq!(parsed[0]["parts"][0]["content"], "The repair is complete.");
        let legacy_completion: Value =
            serde_json::from_str(attrs["gen_ai.completion"].as_str().unwrap())
                .expect("legacy completion should remain JSON");
        assert_eq!(legacy_completion[0]["type"], "text");
        assert_eq!(legacy_completion[0]["text"], "The repair is complete.");
    }

    #[test]
    fn llm_success_span_attrs_bound_legacy_completion() {
        let big = "🎉".repeat(10 * 1024);
        let attrs = llm_success_span_attributes(
            "openai_codex",
            "gpt-5.5",
            "tool_use",
            12_000,
            4_000,
            0,
            0,
            big.len(),
            &json!([
                {"type": "text", "text": big}
            ]),
        );

        let completion = attrs["gen_ai.completion"]
            .as_str()
            .expect("legacy completion should remain a string attribute");
        assert!(completion.ends_with("…[truncated]"));
        assert!(
            completion.len() <= LLM_COMPLETION_ATTR_MAX_BYTES + "…[truncated]".len(),
            "completion attr length {} exceeded expected cap",
            completion.len()
        );
        let _ = std::str::from_utf8(completion.as_bytes()).expect("must remain valid utf-8");
    }

    #[test]
    fn llm_span_hint_headers_use_datadog_genai_semconv() {
        let mut headers = Vec::new();
        append_llm_span_hint_headers(
            &mut headers,
            "anthropic",
            "claude-sonnet-4.6",
            0.7,
            LLM_MAX_TOKENS,
            "You are precise.",
            &[json!({"role": "user", "content": "Summarize the trace."})],
            "session-123",
            "/content/0/text",
        );

        let lookup = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };

        assert_eq!(lookup("X-Temper-Span-Name"), Some("tool.llm_call"));
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.operation.name"),
            Some("chat")
        );
        assert_eq!(
            lookup("X-Temper-Span-Attr-dd_llmobs_enabled"),
            Some("false")
        );
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.provider.name"),
            Some("anthropic")
        );
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.system"),
            Some("anthropic")
        );
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.request.model"),
            Some("claude-sonnet-4.6")
        );
        assert_eq!(
            lookup("X-Temper-Span-Attr-gen_ai.conversation.id"),
            Some("session-123")
        );
        assert_eq!(lookup("X-Temper-Span-Attr-session_id"), Some("session-123"));
        assert_eq!(
            lookup("X-Temper-Span-Attr-tool.name"),
            Some("provider_caller")
        );
        assert!(lookup("X-Temper-Span-Attr-gen_ai.system_instructions").is_some());
        assert!(lookup("X-Temper-Span-Attr-gen_ai.input.messages").is_some());
        assert_eq!(
            lookup("X-Temper-Span-Capture-Response-gen_ai.completion"),
            Some("/content/0/text")
        );
    }

    #[test]
    fn openai_codex_uses_subscription_endpoint_not_public_responses_api() {
        let mut config = std::collections::BTreeMap::new();
        config.insert(
            "openai_api_url".to_string(),
            "https://api.openai.com/v1/responses".to_string(),
        );
        config.insert(
            "openai_codex_api_url".to_string(),
            "https://chatgpt.com/backend-api/codex/responses".to_string(),
        );

        assert_eq!(
            select_openai_responses_url(&config, "openai"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            select_openai_responses_url(&config, "openai_codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn openai_codex_headers_include_chatgpt_account_and_sse_contract() {
        let headers = build_openai_headers("openai_codex", "access-token", Some("acct_123"));

        assert!(headers.contains(&(
            "authorization".to_string(),
            "Bearer access-token".to_string()
        )));
        assert!(headers.contains(&("chatgpt-account-id".to_string(), "acct_123".to_string())));
        assert!(headers.contains(&("OpenAI-Beta".to_string(), "responses=v1".to_string())));
        assert!(headers.contains(&("accept".to_string(), "text/event-stream".to_string())));
    }

    #[test]
    fn public_openai_headers_do_not_include_codex_subscription_headers() {
        let headers = build_openai_headers("openai", "sk-test", Some("acct_123"));

        assert!(!headers.iter().any(|(name, _)| name == "chatgpt-account-id"));
        assert!(!headers.iter().any(|(name, _)| name == "OpenAI-Beta"));
        assert!(headers.contains(&("authorization".to_string(), "Bearer sk-test".to_string())));
    }

    #[test]
    fn openai_responses_input_downgrades_matched_tool_history_to_user_context() {
        let converted = build_openai_responses_input(&[
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "call_ok",
                    "name": "temper_status",
                    "input": {"scope": "dm"}
                }]
            }),
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "call_ok",
                    "content": [{"type": "text", "text": "ready"}]
                }]
            }),
        ]);

        assert_eq!(converted.tool_calls_as_context, 1);
        assert_eq!(converted.tool_outputs_as_context, 1);
        assert!(!converted.input.iter().any(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "function_call_output")
            )
        }));
        assert!(converted.input.iter().any(|item| {
            item.get("role").and_then(Value::as_str) == Some("user")
                && item
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| {
                        content.contains("Tool result for call call_ok")
                            && content.contains("ready")
                    })
        }));
    }

    #[test]
    fn openai_responses_input_downgrades_orphan_tool_outputs_to_user_context() {
        let converted = build_openai_responses_input(&[json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "call_FGEV2z33q2Wz9T03YAVRwx2E",
                "content": [{"type": "text", "text": "status failed"}]
            }]
        })]);

        assert_eq!(converted.tool_calls_as_context, 0);
        assert_eq!(converted.tool_outputs_as_context, 1);
        assert!(!converted.input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
        }));
        assert!(converted.input.iter().any(|item| {
            item.get("role").and_then(Value::as_str) == Some("user")
                && item
                    .get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| {
                        content.contains("Tool result for call")
                            && content.contains("status failed")
                    })
        }));
    }

    #[test]
    fn openai_codex_host_http_failure_message_names_host_boundary() {
        let msg = format_openai_codex_exhausted_error(
            5,
            "HTTP call failed: POST https://chatgpt.com/backend-api/codex/responses",
        );

        assert!(msg.contains("OpenAI Codex host HTTP call failed after 5 attempts"));
        assert!(msg.contains("before a provider HTTP response was returned"));
        assert!(
            msg.contains("HTTP call failed: POST https://chatgpt.com/backend-api/codex/responses")
        );
        assert!(!msg.starts_with("OpenAI Codex API failed"));
    }

    #[test]
    fn fragmented_sse_chunks_reassemble_data_events() {
        let chunks: Vec<&[u8]> = vec![
            b"event: message\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel",
            b"lo\"}\n\n",
            b"data: [DONE]\n\n",
        ];

        let events = collect_sse_data_events(&chunks).expect("SSE chunks should decode");

        assert_eq!(
            events,
            vec![
                "{\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}".to_string(),
                "[DONE]".to_string(),
            ]
        );
    }

    #[test]
    fn openai_stream_chunks_reconstruct_text_usage_and_completion() {
        let chunks: Vec<&[u8]> = vec![
            br#"data: {"type":"response.output_text.delta","delta":"Hel"}"#,
            b"\n\n",
            br#"data: {"type":"response.output_text.delta","delta":"lo"}"#,
            b"\n\n",
            br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":2},"output":[]}}"#,
            b"\n\n",
        ];

        let parsed = parse_openai_stream_chunks(&chunks).expect("OpenAI stream should parse");

        assert_eq!(parsed.content, json!([{"type": "text", "text": "Hello"}]));
        assert_eq!(parsed.stop_reason, "end_turn");
        assert_eq!(parsed.input_tokens, 3);
        assert_eq!(parsed.output_tokens, 2);
        assert_eq!(
            parsed
                .semantic_deltas
                .iter()
                .map(|delta| delta.delta_text.as_str())
                .collect::<Vec<_>>(),
            vec!["Hel", "lo"]
        );
        assert!(parsed.completed);
    }

    #[test]
    fn openai_completed_text_does_not_clobber_streamed_function_call() {
        let chunks: Vec<&[u8]> = vec![
            br#"data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_streamed","name":"execute","arguments":"{\"code\":\"await temper.list('default', 'World')\"}"}}"#,
            b"\n\n",
            br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":9,"output_tokens":4},"output":[{"type":"message","content":[{"type":"output_text","text":"Tool call call_summary: execute({\"code\":\"await temper.list('default', 'World')\"})"}]}]}}"#,
            b"\n\n",
        ];

        let parsed = parse_openai_stream_chunks(&chunks).expect("OpenAI stream should parse");

        assert_eq!(parsed.stop_reason, "tool_use");
        assert_eq!(
            parsed.content,
            json!([
                {"type": "tool_use", "id": "call_streamed", "name": "execute", "input": {"code": "await temper.list('default', 'World')"}},
                {"type": "text", "text": "Tool call call_summary: execute({\"code\":\"await temper.list('default', 'World')\"})"},
            ])
        );
        assert!(parsed.completed);
    }

    #[test]
    fn anthropic_stream_chunks_reconstruct_text_tool_use_and_usage() {
        let chunks: Vec<&[u8]> = vec![
            br#"data: {"type":"message_start","message":{"usage":{"input_tokens":11}}}"#,
            b"\n\n",
            br#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            b"\n\n",
            br#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            b"\n\n",
            br#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"search","input":{}}}"#,
            b"\n\n",
            br#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"cats\"}"}}"#,
            b"\n\n",
            br#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
            b"\n\n",
            br#"data: {"type":"message_stop"}"#,
            b"\n\n",
        ];

        let parsed = parse_anthropic_stream_chunks(&chunks).expect("Anthropic stream should parse");

        assert_eq!(
            parsed.content,
            json!([
                {"type": "text", "text": "Hi"},
                {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "cats"}},
            ])
        );
        assert_eq!(parsed.stop_reason, "tool_use");
        assert_eq!(parsed.input_tokens, 11);
        assert_eq!(parsed.output_tokens, 7);
        assert_eq!(parsed.semantic_deltas[0].delta_text, "Hi");
        assert_eq!(
            parsed.semantic_deltas[1].tool_call_id.as_deref(),
            Some("toolu_1")
        );
        assert!(parsed.completed);
    }

    #[test]
    fn openrouter_stream_chunks_reconstruct_text_tool_calls_and_usage() {
        let chunks: Vec<&[u8]> = vec![
            br#"data: {"choices":[{"delta":{"content":"He"},"finish_reason":null}]}"#,
            b"\n\n",
            br#"data: {"choices":[{"delta":{"content":"y","tool_calls":[{"index":0,"id":"call_1","function":{"name":"lookup","arguments":"{\"id\":"}}]},"finish_reason":null}]}"#,
            b"\n\n",
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"42}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":6}}"#,
            b"\n\n",
            b"data: [DONE]\n\n",
        ];

        let parsed =
            parse_openrouter_stream_chunks(&chunks).expect("OpenRouter stream should parse");

        assert_eq!(
            parsed.content,
            json!([
                {"type": "text", "text": "Hey"},
                {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"id": 42}},
            ])
        );
        assert_eq!(parsed.stop_reason, "tool_use");
        assert_eq!(parsed.input_tokens, 5);
        assert_eq!(parsed.output_tokens, 6);
        assert_eq!(parsed.semantic_deltas[0].delta_text, "He");
        assert_eq!(parsed.semantic_deltas[1].delta_text, "y");
        assert_eq!(
            parsed.semantic_deltas[2].tool_arguments_delta.as_deref(),
            Some("{\"id\":")
        );
        assert_eq!(
            parsed.semantic_deltas[3].tool_arguments_delta.as_deref(),
            Some("42}")
        );
        assert!(parsed.completed);
    }

    #[test]
    fn stream_failures_retry_only_before_semantic_output() {
        assert!(should_retry_stream_failure(1, 5, false));
        assert!(!should_retry_stream_failure(1, 5, true));
        assert!(!should_retry_stream_failure(5, 5, false));
    }

    #[test]
    fn chatgpt_account_id_is_extracted_from_codex_jwt() {
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_456"}}"#;
        let token = format!("header.{}.sig", base64_url_no_pad(payload.as_bytes()));

        assert_eq!(
            extract_chatgpt_account_id_from_jwt(&token).as_deref(),
            Some("acct_456")
        );
    }
}
