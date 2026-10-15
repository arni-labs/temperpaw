//! Small OpenAI-compatible Chat Completions wire helpers.
//!
//! This crate intentionally stays below the orchestration boundary: it builds
//! request/response JSON and parses Chat Completions streams, but it does not
//! perform HTTP, retries, auth, progress dispatch, or entity transitions.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatStreamDelta {
    pub delta_text: String,
    pub accumulated_text_chars: usize,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments_delta: Option<String>,
}

impl ChatStreamDelta {
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

    pub fn is_semantic(&self) -> bool {
        !self.delta_text.is_empty()
            || self.tool_call_id.is_some()
            || self.tool_name.is_some()
            || self.tool_arguments_delta.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedChatCompletion {
    pub content: Vec<Value>,
    pub stop_reason: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub response_bytes: usize,
    pub semantic_deltas: Vec<ChatStreamDelta>,
    pub completed: bool,
    /// Token-level RL signals the serving stack streamed alongside the text
    /// (`logprobs`, `prompt_token_ids`, `completion_token_ids`,
    /// `response_mask`). `None` unless the server actually sent them — the
    /// caller never requests a second round trip to obtain them.
    pub token_signals: Option<Value>,
}

/// Token-level RL signal field names, in the shape OTS turns use.
pub const TOKEN_SIGNAL_FIELDS: &[&str] = &[
    "prompt_token_ids",
    "completion_token_ids",
    "response_mask",
    "logprobs",
];

/// Collapse one stream event's two possible signal homes into a single source.
///
/// Completion-side signals accumulate across events, so a signal taken twice
/// from one event is stored twice — and when only one signal is present nothing
/// downstream can detect it, because there is no second array to disagree on
/// length. Every accumulator routes an event through this before merging,
/// whatever its wire shape.
///
/// `content` is the level carrying the model's output for that event: the
/// choice in a chat-completions chunk, the response in a Responses
/// `response.completed`. `usage` is the token-accounting object beside it.
/// Where both carry the same field the content level wins — `usage` repeating a
/// signal is a server quirk, not a second measurement. `None` when neither
/// level carries any signal field.
pub fn event_token_signals(usage: Option<&Value>, content: Option<&Value>) -> Option<Value> {
    let mut source = Map::new();
    for field in TOKEN_SIGNAL_FIELDS {
        let value = content
            .and_then(|content| content.get(*field))
            .or_else(|| usage.and_then(|usage| usage.get(*field)));
        if let Some(value) = value {
            source.insert((*field).to_string(), value.clone());
        }
    }
    (!source.is_empty()).then(|| Value::Object(source))
}

/// Signals that describe the prompt, which does not grow while the completion
/// streams. Repeating them on every chunk is common, so they are set once and
/// never concatenated — the completion-side signals are the ones that append.
const SET_ONCE_TOKEN_SIGNALS: &[&str] = &["prompt_token_ids"];

/// Merge any token-level RL signals found in `source` into `signals`.
///
/// Completion-side signals accumulate across streamed chunks, because a
/// chat-completions server emits them one delta at a time; prompt-side signals
/// are set once. Every field is normalized to the flat array the OTS contract
/// requires: `logprobs` arrives from OpenAI-compatible servers as
/// `{"content": [{"token": …, "logprob": …}]}` and is flattened to the bare
/// logprob values. Shapes that cannot be normalized are ignored rather than
/// guessed at.
pub fn merge_token_signals(signals: &mut Option<Value>, source: &Value) {
    for field in TOKEN_SIGNAL_FIELDS {
        let Some(raw) = source.get(*field) else {
            continue;
        };
        let incoming = if *field == "logprobs" {
            normalize_logprobs(raw)
        } else {
            // Token ids and mask bits are numbers. An endpoint streaming
            // anything else under these names is not producing the signal the
            // field names, and passing it through would put unbounded foreign
            // text on a SessionEntry whose budget assumes numbers — and would
            // be dropped by the emitter's own shape checks anyway.
            raw.as_array()
                .filter(|items| items.iter().all(Value::is_number))
                .cloned()
        };
        let Some(incoming) = incoming.filter(|items| !items.is_empty()) else {
            continue;
        };
        let map = signals.get_or_insert_with(|| Value::Object(Map::new()));
        let Some(map) = map.as_object_mut() else {
            return;
        };
        if SET_ONCE_TOKEN_SIGNALS.contains(field) {
            map.entry((*field).to_string())
                .or_insert_with(|| Value::Array(incoming));
            continue;
        }
        map.entry((*field).to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(existing) = map.get_mut(*field).and_then(Value::as_array_mut) {
            existing.extend(incoming);
        }
    }
}

/// Flatten an OpenAI-compatible `logprobs` payload to bare logprob values.
fn normalize_logprobs(raw: &Value) -> Option<Vec<Value>> {
    if let Some(items) = raw.as_array() {
        if items.iter().all(Value::is_number) {
            return Some(items.clone());
        }
        return flatten_logprob_entries(items);
    }
    raw.get("content")
        .and_then(Value::as_array)
        .and_then(|items| flatten_logprob_entries(items))
}

/// Take the `logprob` of every entry, or nothing.
///
/// The flattened array is positional: element *i* is the probability of
/// completion token *i*. Dropping one malformed entry would shift every later
/// probability onto the wrong token, which is worse than having no
/// probabilities at all — so a payload that cannot be flattened whole is
/// rejected whole.
fn flatten_logprob_entries(items: &[Value]) -> Option<Vec<Value>> {
    items
        .iter()
        .map(|item| item.get("logprob").filter(|value| value.is_number()).cloned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatStreamParseFailure {
    pub message: String,
    pub semantic_output_seen: bool,
}

impl ChatStreamParseFailure {
    fn new(message: impl Into<String>, semantic_output_seen: bool) -> Self {
        Self {
            message: message.into(),
            semantic_output_seen,
        }
    }
}

impl std::fmt::Display for ChatStreamParseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Default)]
struct ChatToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub struct ChatCompletionStreamAccumulator {
    text: String,
    tool_calls: BTreeMap<usize, ChatToolCallAccum>,
    finish_reason: String,
    input_tokens: i64,
    output_tokens: i64,
    saw_done: bool,
    semantic_deltas: Vec<ChatStreamDelta>,
    token_signals: Option<Value>,
    response_id: String,
}

impl ChatCompletionStreamAccumulator {
    pub fn ingest_data(
        &mut self,
        data: &str,
    ) -> Result<Vec<ChatStreamDelta>, ChatStreamParseFailure> {
        if data.trim() == "[DONE]" {
            self.saw_done = true;
            return Ok(Vec::new());
        }

        let event: Value = serde_json::from_str(data).map_err(|err| {
            ChatStreamParseFailure::new(
                format!("parse OpenAI-compatible chat stream event: {err}"),
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
                self.ingest_message_like(delta, &mut deltas);
            } else if let Some(message) = choice.get("message") {
                self.ingest_message_like(message, &mut deltas);
            }
        }

        self.semantic_deltas.extend(deltas.clone());
        Ok(deltas)
    }

    fn ingest_message_like(&mut self, value: &Value, deltas: &mut Vec<ChatStreamDelta>) {
        if let Some(text) = value.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            self.text.push_str(text);
            deltas.push(ChatStreamDelta::text(text, self.text.chars().count()));
        }
        ingest_tool_call_deltas(
            value,
            &mut self.tool_calls,
            self.text.chars().count(),
            deltas,
        );
    }

    pub fn semantic_output_seen(&self) -> bool {
        !self.text.is_empty()
            || !self.tool_calls.is_empty()
            || self
                .semantic_deltas
                .iter()
                .any(ChatStreamDelta::is_semantic)
    }

    pub fn finalize(
        self,
        response_bytes: usize,
    ) -> Result<ParsedChatCompletion, ChatStreamParseFailure> {
        if !self.saw_done && self.finish_reason.is_empty() {
            return Err(ChatStreamParseFailure::new(
                "OpenAI-compatible chat stream ended before [DONE] or finish_reason",
                self.semantic_output_seen(),
            ));
        }

        Ok(ParsedChatCompletion {
            content: chat_content_blocks(&self.text, &self.tool_calls, &self.response_id),
            stop_reason: if !self.tool_calls.is_empty() {
                "tool_use".to_string()
            } else {
                "end_turn".to_string()
            },
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            response_bytes,
            semantic_deltas: self.semantic_deltas,
            completed: true,
            token_signals: self.token_signals,
        })
    }
}

fn ingest_tool_call_deltas(
    value: &Value,
    tool_calls: &mut BTreeMap<usize, ChatToolCallAccum>,
    accumulated_text_chars: usize,
    deltas: &mut Vec<ChatStreamDelta>,
) {
    let Some(calls) = value.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for tool_call in calls {
        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let accum = tool_calls.entry(index).or_default();
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
        deltas.push(ChatStreamDelta::tool(
            (!accum.id.is_empty()).then(|| accum.id.clone()),
            (!accum.name.is_empty()).then(|| accum.name.clone()),
            (!args_delta.is_empty()).then(|| args_delta.to_string()),
            accumulated_text_chars,
        ));
    }
}

/// Tool-call id for a provider that sent none.
///
/// A bare positional id (`tool_1`) restarts at every response, so two turns that
/// each make one call end up sharing an id — and anything that indexes calls by
/// id across a session (the OTS emitter's decisions and tool spans, the wire
/// conversion back to chat format) then collapses them into one. The response id
/// scopes the fallback to the completion that produced it.
pub fn synthetic_tool_call_id(response_id: &str, scope: &str, position: usize) -> String {
    if response_id.is_empty() {
        format!("{scope}_{position}")
    } else {
        format!("{response_id}_{scope}_{position}")
    }
}

fn chat_content_blocks(
    text: &str,
    tool_calls: &BTreeMap<usize, ChatToolCallAccum>,
    response_id: &str,
) -> Vec<Value> {
    let mut content = Vec::<Value>::new();
    if !text.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text,
        }));
    }
    for (idx, tool_call) in tool_calls {
        let input = if tool_call.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&tool_call.arguments)
                .unwrap_or_else(|_| json!({ "raw": tool_call.arguments }))
        };
        content.push(json!({
            "type": "tool_use",
            "id": if tool_call.id.is_empty() {
                synthetic_tool_call_id(response_id, "tool", idx + 1)
            } else {
                tool_call.id.clone()
            },
            "name": if tool_call.name.is_empty() { "unknown_tool" } else { &tool_call.name },
            "input": input,
        }));
    }
    content
}

pub fn parse_chat_completion_stream_events(
    events: &[String],
    response_bytes: usize,
) -> Result<ParsedChatCompletion, ChatStreamParseFailure> {
    let mut acc = ChatCompletionStreamAccumulator::default();
    for event in events {
        acc.ingest_data(event)?;
    }
    acc.finalize(response_bytes)
}

pub fn parse_chat_completion_response_text(body: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|err| format!("parse chat completion JSON: {err}"))?;
    if let Some(error) = value.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("OpenAI-compatible chat error")
            .to_string());
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "No choices[0].message.content in chat completion response".to_string())
}

pub fn build_chat_completion_body(
    model: &str,
    system_prompt: &str,
    messages: &[Value],
    tools: &[Value],
    max_tokens: i64,
    temperature: f64,
    stream: bool,
    include_usage: bool,
    provider_options_json: &str,
) -> Result<Value, String> {
    let mut body = json!({
        "model": model,
        "messages": convert_messages_to_chat(system_prompt, messages),
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": stream,
    });
    if stream && include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }

    let chat_tools = convert_tools_to_chat(tools);
    if !chat_tools.is_empty() {
        body["tools"] = json!(chat_tools);
        body["tool_choice"] = json!("auto");
    }

    merge_provider_options(&mut body, provider_options_json)?;
    Ok(body)
}

pub fn merge_provider_options(body: &mut Value, provider_options_json: &str) -> Result<(), String> {
    let options = provider_options_json.trim();
    if options.is_empty() {
        return Ok(());
    }
    let parsed: Value = serde_json::from_str(options)
        .map_err(|err| format!("provider_options_json must be a JSON object: {err}"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| "provider_options_json must be a JSON object".to_string())?;
    let Some(body_object) = body.as_object_mut() else {
        return Err("chat completion body must be an object".to_string());
    };
    for (key, value) in object {
        if is_reserved_provider_option_key(key) {
            return Err(format!("provider_options_json key '{key}' is reserved"));
        }
        body_object.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn is_reserved_provider_option_key(key: &str) -> bool {
    matches!(key, "messages" | "tools" | "stream" | "model")
}

pub fn convert_messages_to_chat(system_prompt: &str, messages: &[Value]) -> Vec<Value> {
    let mut out = Vec::<Value>::new();
    if !system_prompt.trim().is_empty() {
        out.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }
    for (message_index, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = msg.get("content").cloned().unwrap_or(json!(""));

        match content {
            Value::String(text) => out.push(json!({ "role": role, "content": text })),
            Value::Array(blocks) if role == "assistant" => {
                let mut text_chunks = Vec::<String>::new();
                let mut tool_calls = Vec::<Value>::new();
                for (idx, block) in blocks.iter().enumerate() {
                    match block.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                text_chunks.push(text.to_string());
                            }
                        }
                        "tool_use" => {
                            // Position within the message is not unique across a
                            // conversation; scope it so two id-less assistant
                            // turns cannot send the provider the same call id.
                            let id = block
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| {
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
                            let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
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
            }
            Value::Array(blocks) if role == "user" => append_user_blocks(&mut out, &blocks),
            other => out.push(json!({ "role": role, "content": other })),
        }
    }
    out
}

fn append_user_blocks(out: &mut Vec<Value>, blocks: &[Value]) {
    let mut user_text = Vec::<String>::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("") {
            "tool_result" => {
                let tool_call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_tool_call");
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": stringify_content(block.get("content").unwrap_or(&Value::Null)),
                }));
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
        out.push(json!({ "role": "user", "content": user_text.join("\n") }));
    }
}

fn stringify_content(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    Some(text.to_string())
                } else if let Some(text) = item.as_str() {
                    Some(text.to_string())
                } else {
                    serde_json::to_string(item).ok()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub fn convert_tools_to_chat(tools: &[Value]) -> Vec<Value> {
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
            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
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

pub fn parse_headers_json(headers_json: &str) -> Result<Vec<(String, String)>, String> {
    let headers_json = headers_json.trim();
    if headers_json.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Value = serde_json::from_str(headers_json)
        .map_err(|err| format!("openai_compatible_headers_json must be a JSON object: {err}"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| "openai_compatible_headers_json must be a JSON object".to_string())?;
    let mut headers = Vec::new();
    for (key, value) in object {
        validate_header_name(key)?;
        let Some(value) = value.as_str() else {
            return Err(format!("header '{key}' must have a string value"));
        };
        if value.contains('\r') || value.contains('\n') {
            return Err(format!("header '{key}' contains a newline"));
        }
        headers.push((key.clone(), value.to_string()));
    }
    Ok(headers)
}

fn validate_header_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("header names cannot be empty".to_string());
    }
    if name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("content-type")
        || name.eq_ignore_ascii_case("accept")
    {
        return Err(format!("header '{name}' is managed by TemperPaw"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!("header '{name}' contains unsupported characters"));
    }
    Ok(())
}

#[allow(dead_code)]
fn map_from_pairs(pairs: &[(String, String)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_token_signals_flattens_openai_logprobs() {
        let mut signals = None;
        merge_token_signals(
            &mut signals,
            &json!({
                "logprobs": { "content": [
                    {"token": "he", "logprob": -0.25},
                    {"token": "llo", "logprob": -1.5}
                ]}
            }),
        );
        assert_eq!(signals.unwrap()["logprobs"], json!([-0.25, -1.5]));
    }

    #[test]
    fn merge_token_signals_accumulates_across_chunks() {
        let mut signals = None;
        merge_token_signals(&mut signals, &json!({ "logprobs": [-0.1] }));
        merge_token_signals(&mut signals, &json!({ "logprobs": [-0.2] }));
        merge_token_signals(
            &mut signals,
            &json!({ "completion_token_ids": [7, 8], "response_mask": [1, 1] }),
        );
        let signals = signals.unwrap();
        assert_eq!(signals["logprobs"], json!([-0.1, -0.2]));
        assert_eq!(signals["completion_token_ids"], json!([7, 8]));
        assert_eq!(signals["response_mask"], json!([1, 1]));
    }

    #[test]
    fn merge_token_signals_sets_prompt_ids_once_instead_of_concatenating() {
        // A server that repeats the prompt ids on every chunk must not end up
        // with the prompt counted N times.
        let mut signals = None;
        for _ in 0..4 {
            merge_token_signals(&mut signals, &json!({ "prompt_token_ids": [11, 12, 13] }));
        }
        assert_eq!(signals.unwrap()["prompt_token_ids"], json!([11, 12, 13]));
    }

    /// Logprobs are positional: element *i* belongs to completion token *i*.
    /// Skipping a malformed entry shifts every later probability onto the wrong
    /// token and leaves the arrays at different lengths, so the payload is
    /// dropped whole instead.
    #[test]
    fn merge_token_signals_drops_logprobs_that_cannot_be_flattened_whole() {
        let mut signals = None;
        merge_token_signals(
            &mut signals,
            &json!({
                "completion_token_ids": [7, 8, 9],
                "response_mask": [1, 1, 1],
                "logprobs": { "content": [
                    {"token": "a", "logprob": -0.1},
                    {"token": "b"},
                    {"token": "c", "logprob": -0.3}
                ]},
            }),
        );
        let signals = signals.expect("token id signals still recorded");
        assert_eq!(signals["completion_token_ids"], json!([7, 8, 9]));
        assert!(
            signals.get("logprobs").is_none(),
            "a partial logprob array must never be stored alongside full token ids"
        );
    }

    /// Token ids and mask bits are numbers. An OpenAI-compatible endpoint is
    /// configurable per agent, so what it streams under these names is not
    /// trusted: text elements here would be unbounded foreign content written
    /// onto a SessionEntry whose size budget assumes numbers, and the emitter's
    /// own shape checks would drop them from the trajectory anyway.
    #[test]
    fn merge_token_signals_rejects_non_numeric_token_arrays() {
        let mut signals = None;
        merge_token_signals(
            &mut signals,
            &json!({
                "prompt_token_ids": ["hello", "world"],
                "completion_token_ids": [7, 8],
                "response_mask": [1, "0"],
            }),
        );
        let signals = signals.expect("the numeric signal is still recorded");
        assert_eq!(signals["completion_token_ids"], json!([7, 8]));
        assert!(
            signals.get("prompt_token_ids").is_none()
                && signals.get("response_mask").is_none(),
            "arrays carrying anything but numbers must be dropped whole: {signals}"
        );
    }

    /// A server that echoes the same payload under both `usage` and
    /// `choices[0]` of one event must not have it counted twice. With a single
    /// signal present there is no second array to disagree on length, so the
    /// doubling would reach an RL consumer as real token ids.
    #[test]
    fn one_event_contributes_each_token_signal_once() {
        let mut acc = ChatCompletionStreamAccumulator::default();
        acc.ingest_data(
            &json!({
                "usage": {"completion_token_ids": [7, 8]},
                "choices": [{"completion_token_ids": [7, 8], "delta": {"content": "hi"}}],
            })
            .to_string(),
        )
        .expect("event parses");

        let signals = acc.token_signals.clone().expect("signals recorded");
        assert_eq!(
            signals["completion_token_ids"],
            json!([7, 8]),
            "the repeated payload must be taken once, not concatenated"
        );
    }

    /// Where both levels carry the same field the per-choice value wins, and a
    /// field only `usage` carries is still picked up.
    #[test]
    fn choice_level_token_signals_win_over_usage_level() {
        let source = event_token_signals(
            Some(&json!({"completion_token_ids": [1], "prompt_token_ids": [5, 6]})),
            Some(&json!({"completion_token_ids": [9]})),
        )
        .expect("a source is produced");

        assert_eq!(source["completion_token_ids"], json!([9]), "choice wins");
        assert_eq!(
            source["prompt_token_ids"],
            json!([5, 6]),
            "a field only usage carries is still taken"
        );
        assert!(event_token_signals(None, None).is_none());
        assert!(event_token_signals(Some(&json!({"prompt_tokens": 4})), None).is_none());
    }

    /// Across events the completion side still accumulates — that is what makes
    /// a streamed completion whole.
    #[test]
    fn completion_signals_still_accumulate_across_events() {
        let mut acc = ChatCompletionStreamAccumulator::default();
        for chunk in [json!({"choices": [{"completion_token_ids": [1]}]}),
                      json!({"choices": [{"completion_token_ids": [2]}]})] {
            acc.ingest_data(&chunk.to_string()).expect("event parses");
        }
        let signals = acc.token_signals.clone().expect("signals recorded");
        assert_eq!(signals["completion_token_ids"], json!([1, 2]));
    }

    #[test]
    fn merge_token_signals_stays_none_for_providers_that_send_nothing() {
        let mut signals = None;
        merge_token_signals(
            &mut signals,
            &json!({ "usage": {"prompt_tokens": 10}, "logprobs": null }),
        );
        assert!(signals.is_none());
    }

    #[test]
    fn chat_stream_accumulator_captures_streamed_logprobs() {
        let mut accumulator = ChatCompletionStreamAccumulator::default();
        accumulator
            .ingest_data(
                r#"{"choices":[{"delta":{"content":"hi"},"logprobs":{"content":[{"token":"hi","logprob":-0.5}]}}]}"#,
            )
            .expect("chunk parses");
        accumulator
            .ingest_data(
                r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"prompt_token_ids":[11,12,13]}}"#,
            )
            .expect("final chunk parses");
        let parsed = accumulator.finalize(128).expect("stream finalizes");
        let signals = parsed.token_signals.expect("signals captured");
        assert_eq!(signals["logprobs"], json!([-0.5]));
        assert_eq!(signals["prompt_token_ids"], json!([11, 12, 13]));
    }

    #[test]
    fn chat_stream_accumulator_leaves_signals_absent_without_them() {
        let mut accumulator = ChatCompletionStreamAccumulator::default();
        accumulator
            .ingest_data(r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#)
            .expect("chunk parses");
        let parsed = accumulator.finalize(64).expect("stream finalizes");
        assert!(parsed.token_signals.is_none());
    }

    #[test]
    fn builds_chat_body_and_merges_safe_provider_options() {
        let body = build_chat_completion_body(
            "openrouter/fusion",
            "system",
            &[json!({"role": "user", "content": "hello"})],
            &[json!({
                "name": "search",
                "description": "Search",
                "input_schema": {"type": "object"}
            })],
            1024,
            0.7,
            true,
            true,
            r#"{"plugins":[{"id":"fusion","preset":"general-budget"}],"provider":{"order":["Fireworks"]}}"#,
        )
        .expect("safe options should merge");

        assert_eq!(body["model"], "openrouter/fusion");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["tools"][0]["function"]["name"], "search");
        assert_eq!(body["plugins"][0]["id"], "fusion");
        assert_eq!(body["provider"]["order"][0], "Fireworks");
    }

    #[test]
    fn rejects_reserved_provider_options() {
        let err = build_chat_completion_body(
            "m",
            "",
            &[],
            &[],
            1,
            1.0,
            true,
            true,
            r#"{"messages":[]}"#,
        )
        .expect_err("messages must be reserved");
        assert!(err.contains("reserved"));
    }

    #[test]
    fn parses_streaming_text_tool_calls_and_usage() {
        let events = vec![
            r#"{"choices":[{"delta":{"content":"hi "}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"temper_list","arguments":"{\"kind\""}}]}}]}"#.to_string(),
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"Session\"}"}}]}}]}"#.to_string(),
            r#"{"choices":[{"finish_reason":"tool_calls","delta":{}}],"usage":{"prompt_tokens":12,"completion_tokens":7}}"#.to_string(),
            "[DONE]".to_string(),
        ];

        let parsed = parse_chat_completion_stream_events(&events, 512).expect("stream parses");
        assert_eq!(parsed.input_tokens, 12);
        assert_eq!(parsed.output_tokens, 7);
        assert_eq!(parsed.stop_reason, "tool_use");
        assert_eq!(parsed.content[0]["text"], "hi ");
        assert_eq!(parsed.content[1]["name"], "temper_list");
        assert_eq!(parsed.content[1]["input"]["kind"], "Session");
    }

    /// A provider that omits tool-call ids used to get `tool_1` on every turn,
    /// so a session's second call overwrote the first everywhere calls are keyed
    /// by id — OTS decisions, tool spans, and the wire conversion back to chat.
    #[test]
    fn synthetic_tool_call_ids_are_scoped_to_their_response() {
        let call_without_id = |completion: &str| {
            let events = vec![
                format!(
                    r#"{{"id":"{completion}","choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"name":"temper_list","arguments":"{{}}"}}}}]}}}}]}}"#
                ),
                r#"{"choices":[{"finish_reason":"tool_calls","delta":{}}]}"#.to_string(),
                "[DONE]".to_string(),
            ];
            parse_chat_completion_stream_events(&events, 64).expect("stream parses").content[0]
                ["id"]
                .as_str()
                .expect("tool_use id")
                .to_string()
        };

        let first = call_without_id("chatcmpl-aaa");
        let second = call_without_id("chatcmpl-bbb");
        assert_eq!(first, "chatcmpl-aaa_tool_1");
        assert_ne!(
            first, second,
            "two responses that both omit call ids must not share one"
        );
    }

    #[test]
    fn synthetic_tool_call_ids_fall_back_when_the_response_has_no_id() {
        assert_eq!(synthetic_tool_call_id("", "tool", 1), "tool_1");
        assert_eq!(synthetic_tool_call_id("gen-9", "or_tool", 2), "gen-9_or_tool_2");
    }

    /// Two assistant turns that both lost their call ids must not send the
    /// provider one ambiguous id shared by both.
    #[test]
    fn converted_messages_scope_missing_tool_call_ids_per_message() {
        let messages = vec![
            json!({"role":"assistant","content":[{"type":"tool_use","name":"a","input":{}}]}),
            json!({"role":"assistant","content":[{"type":"tool_use","name":"b","input":{}}]}),
        ];
        let converted = convert_messages_to_chat("", &messages);
        let ids: Vec<&str> = converted
            .iter()
            .filter_map(|message| message.get("tool_calls"))
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(|call| call["id"].as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn parses_non_stream_chat_completion_content() {
        let text = parse_chat_completion_response_text(
            r#"{"choices":[{"message":{"content":"compact summary"}}],"usage":{"prompt_tokens":1}}"#,
        )
        .expect("response parses");
        assert_eq!(text, "compact summary");
    }

    #[test]
    fn parses_safe_extra_headers() {
        let headers =
            parse_headers_json(r#"{"X-Provider-Key":"abc","x-team":"paw"}"#).expect("headers");
        assert_eq!(headers.len(), 2);
        assert!(parse_headers_json(r#"{"Authorization":"no"}"#).is_err());
        assert!(parse_headers_json(r#"{"Bad Header":"no"}"#).is_err());
    }
}
