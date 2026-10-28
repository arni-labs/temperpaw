use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedContextArtifact {
    pub version: u32,
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub system_prompt: String,
    pub system_prompt_hash: String,
    pub system_prompt_file_id: String,
    pub conversation_file_id: String,
    pub session_file_id: String,
    pub session_leaf_id: String,
    pub workspace_id: String,
    pub use_session_tree: bool,
    pub context_tokens: usize,
    pub context_bytes: usize,
    pub entries_loaded: usize,
    pub content_files_loaded: usize,
    #[serde(default = "default_prune_tool_results_after_turns")]
    pub prune_tool_results_after_turns: usize,
}

fn default_prune_tool_results_after_turns() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponseArtifact {
    pub version: u32,
    pub provider: String,
    pub model: String,
    pub content: Value,
    pub stop_reason: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub request_bytes: usize,
    pub response_bytes: usize,
    /// Token-level RL signals the serving stack returned with this completion
    /// (`prompt_token_ids`, `completion_token_ids`, `response_mask`,
    /// `logprobs`). Absent for providers that do not expose them — the agent
    /// never makes an extra round trip to obtain them. See ADR-0035 section 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_signals: Option<Value>,
}

/// Field names of the token-level RL signals carried between the provider
/// response and the OTS turn. The kernel's `OTSTurn` uses the same names.
pub const TOKEN_SIGNAL_FIELDS: &[&str] = &[
    "prompt_token_ids",
    "completion_token_ids",
    "response_mask",
    "logprobs",
];

/// Pick the token-level RL signals out of a raw provider payload.
///
/// Returns `None` when the payload carries none of them, which is the normal
/// case for the Anthropic-shaped providers this stack talks to today.
pub fn extract_token_signals(raw: &Value) -> Option<Value> {
    let mut signals = serde_json::Map::new();
    for field in TOKEN_SIGNAL_FIELDS {
        let value = raw
            .get(*field)
            .or_else(|| raw.get("usage").and_then(|usage| usage.get(*field)))
            .or_else(|| raw.get("choices").and_then(|choices| {
                choices
                    .as_array()
                    .and_then(|items| items.first())
                    .and_then(|choice| choice.get(*field))
            }));
        if let Some(value) = value.filter(|value| value.is_array()) {
            signals.insert((*field).to_string(), value.clone());
        }
    }
    (!signals.is_empty()).then(|| Value::Object(signals))
}

pub fn parse_prepared_context_artifact(raw: &str) -> Result<PreparedContextArtifact, String> {
    parse_artifact_json(raw, "prepared context artifact")
}

pub fn parse_provider_response_artifact(raw: &str) -> Result<ProviderResponseArtifact, String> {
    parse_artifact_json(raw, "provider response artifact")
}

fn parse_artifact_json<T>(raw: &str, label: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let value = parse_artifact_value(raw, label)?;
    serde_json::from_value(value).map_err(|err| format!("parse {label}: {err}"))
}

fn parse_artifact_value(raw: &str, label: &str) -> Result<Value, String> {
    let mut current = raw.trim().to_string();

    for _ in 0..3 {
        let value: Value =
            serde_json::from_str(&current).map_err(|err| format!("parse {label}: {err}"))?;
        match value {
            Value::String(inner) => {
                current = inner.trim().to_string();
            }
            other => return Ok(other),
        }
    }

    Err(format!("parse {label}: nested JSON string exceeded limit"))
}

pub fn build_provider_response_ready_params(
    provider_response_file_id: &str,
    prepared: &PreparedContextArtifact,
    artifact: &ProviderResponseArtifact,
) -> Value {
    build_provider_response_ready_params_with_inline(
        provider_response_file_id,
        "",
        prepared,
        artifact,
    )
}

pub fn build_provider_response_ready_params_with_inline(
    provider_response_file_id: &str,
    provider_response_inline_json: &str,
    prepared: &PreparedContextArtifact,
    artifact: &ProviderResponseArtifact,
) -> Value {
    json!({
        "provider_response_file_id": provider_response_file_id,
        "provider_response_inline_json": provider_response_inline_json,
        "provider_request_bytes": artifact.request_bytes,
        "provider_response_bytes": artifact.response_bytes,
        "input_tokens": artifact.input_tokens,
        "output_tokens": artifact.output_tokens,
        "_gen_ai_system_instructions": build_gen_ai_system_instructions(&prepared.system_prompt),
        "_gen_ai_input_messages": build_gen_ai_input_messages(&prepared.messages),
        "_gen_ai_output_messages": build_gen_ai_output_messages(
            &artifact.content,
            &artifact.stop_reason,
        ),
        "_gen_ai_provider": artifact.provider,
        "_gen_ai_model": artifact.model,
        "_gen_ai_finish_reason": artifact.stop_reason,
    })
}

pub fn build_provider_response_applier_base_params(
    prepared: &PreparedContextArtifact,
    artifact: &ProviderResponseArtifact,
) -> Value {
    json!({
        "provider_response_file_id": "",
        "provider_response_inline_json": "",
        "input_tokens": artifact.input_tokens,
        "output_tokens": artifact.output_tokens,
        "system_prompt_hash": prepared.system_prompt_hash,
        "system_prompt_file_id": prepared.system_prompt_file_id,
        "provider_request_bytes": artifact.request_bytes,
        "provider_response_bytes": artifact.response_bytes,
    })
}

const GEN_AI_MESSAGE_ATTR_LIMIT: usize = 16_384;
const GEN_AI_COMPACT_TEXT_LIMIT: usize = 2_048;
const GEN_AI_COMPACT_JSON_LIMIT: usize = 3_072;
const GEN_AI_COMPACT_KEEP_COUNTS: &[usize] = &[32, 16, 8, 4, 2, 1];

pub fn build_gen_ai_system_instructions(system_prompt: &str) -> String {
    let payload = if system_prompt.is_empty() {
        json!([])
    } else {
        json!([{
            "type": "text",
            "content": truncate_for_gen_ai_attr(system_prompt),
        }])
    };

    serialize_gen_ai_payload(payload, |size| {
        json!([{
            "type": "text",
            "content": format!("[truncated, {size} bytes]"),
        }])
    })
}

pub fn build_gen_ai_input_messages(messages: &[Value]) -> String {
    let mut gen_ai_msgs = Vec::new();

    for message in messages {
        match message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
        {
            "user" => append_user_gen_ai_messages(&mut gen_ai_msgs, message.get("content")),
            "assistant" => {
                if let Some(parts) = build_assistant_parts(message.get("content")) {
                    gen_ai_msgs.push(json!({
                        "role": "assistant",
                        "parts": parts,
                    }));
                }
            }
            "tool" | "tool_result" => {
                if let Some(tool_message) =
                    build_tool_response_message(message.get("tool_use_id"), message.get("content"))
                {
                    gen_ai_msgs.push(tool_message);
                }
            }
            other => {
                if let Some(content) = message.get("content") {
                    let text = truncate_for_gen_ai_attr(&stringify_content(content));
                    if !text.is_empty() {
                        gen_ai_msgs.push(json!({
                            "role": other,
                            "parts": [{"type": "text", "content": text}],
                        }));
                    }
                }
            }
        }
    }

    serialize_gen_ai_messages_payload(gen_ai_msgs)
}

pub fn build_gen_ai_output_messages(response_content: &Value, finish_reason: &str) -> String {
    let mut parts = Vec::new();
    if let Some(blocks) = response_content.as_array() {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(json!({
                            "type": "text",
                            "content": truncate_for_gen_ai_attr(text),
                        }));
                    }
                }
                "tool_use" => {
                    parts.push(json!({
                        "type": "tool_call",
                        "id": block.get("id").and_then(Value::as_str).unwrap_or_default(),
                        "name": block.get("name").and_then(Value::as_str).unwrap_or("unknown"),
                        "arguments": block.get("input").cloned().unwrap_or_else(|| json!({})),
                    }));
                }
                _ => {}
            }
        }
    }

    if !parts_have_text_content(&parts) {
        if let Some(summary) = summarize_tool_call_parts(&parts) {
            parts.insert(
                0,
                json!({
                    "type": "text",
                    "content": summary,
                }),
            );
        }
    }

    let payload = if parts.is_empty() {
        Vec::new()
    } else {
        vec![json!({
            "role": "assistant",
            "finish_reason": finish_reason,
            "parts": parts,
        })]
    };

    serialize_gen_ai_messages_payload(payload)
}

fn append_user_gen_ai_messages(target: &mut Vec<Value>, content: Option<&Value>) {
    let Some(content) = content else {
        return;
    };

    match content {
        Value::String(text) => {
            let text = truncate_for_gen_ai_attr(text);
            if !text.is_empty() {
                target.push(json!({
                    "role": "user",
                    "parts": [{"type": "text", "content": text}],
                }));
            }
        }
        Value::Array(blocks) => {
            let mut user_parts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            user_parts.push(json!({
                                "type": "text",
                                "content": truncate_for_gen_ai_attr(text),
                            }));
                        }
                    }
                    "tool_result" => {
                        if let Some(message) = build_tool_response_message(
                            block.get("tool_use_id"),
                            block.get("content"),
                        ) {
                            target.push(message);
                        }
                    }
                    _ => {}
                }
            }

            if !user_parts.is_empty() {
                target.push(json!({
                    "role": "user",
                    "parts": user_parts,
                }));
            }
        }
        other => {
            let text = truncate_for_gen_ai_attr(&stringify_content(other));
            if !text.is_empty() {
                target.push(json!({
                    "role": "user",
                    "parts": [{"type": "text", "content": text}],
                }));
            }
        }
    }
}

fn build_assistant_parts(content: Option<&Value>) -> Option<Vec<Value>> {
    let content = content?;
    let mut parts = Vec::new();

    match content {
        Value::String(text) => {
            let text = truncate_for_gen_ai_attr(text);
            if !text.is_empty() {
                parts.push(json!({"type": "text", "content": text}));
            }
        }
        Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(json!({
                                "type": "text",
                                "content": truncate_for_gen_ai_attr(text),
                            }));
                        }
                    }
                    "tool_use" => {
                        parts.push(json!({
                            "type": "tool_call",
                            "id": block.get("id").and_then(Value::as_str).unwrap_or_default(),
                            "name": block.get("name").and_then(Value::as_str).unwrap_or("unknown"),
                            "arguments": block.get("input").cloned().unwrap_or_else(|| json!({})),
                        }));
                    }
                    _ => {}
                }
            }
        }
        other => {
            let text = truncate_for_gen_ai_attr(&stringify_content(other));
            if !text.is_empty() {
                parts.push(json!({"type": "text", "content": text}));
            }
        }
    }

    (!parts.is_empty()).then_some(parts)
}

fn build_tool_response_message(
    tool_use_id: Option<&Value>,
    content: Option<&Value>,
) -> Option<Value> {
    let tool_use_id = tool_use_id.and_then(Value::as_str)?;
    let result = normalize_tool_result_content(content?);

    Some(json!({
        "role": "tool",
        "id": tool_use_id,
        "parts": [{
            "type": "tool_call_response",
            "id": tool_use_id,
            "result": result,
        }],
    }))
}

fn normalize_tool_result_content(content: &Value) -> Value {
    match content {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .unwrap_or_else(|_| json!(truncate_for_gen_ai_attr(text))),
        Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            parts.push(t.to_string());
                        }
                    }
                    "image" => {
                        let media_type = block
                            .get("source")
                            .and_then(|s| s.get("media_type"))
                            .and_then(Value::as_str)
                            .unwrap_or("image/unknown");
                        parts.push(format!("[image: {media_type}]"));
                    }
                    _ => {}
                }
            }
            let combined = parts.join("\n");
            if combined.is_empty() {
                json!(truncate_for_gen_ai_attr(&content.to_string()))
            } else {
                json!(truncate_for_gen_ai_attr(&combined))
            }
        }
        other => other.clone(),
    }
}

fn serialize_gen_ai_payload(payload: Value, fallback: impl Fn(usize) -> Value) -> String {
    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    if serialized.len() <= GEN_AI_MESSAGE_ATTR_LIMIT {
        serialized
    } else {
        serde_json::to_string(&fallback(serialized.len())).unwrap_or_else(|_| "[]".to_string())
    }
}

fn serialize_gen_ai_messages_payload(messages: Vec<Value>) -> String {
    let payload = json!(messages);
    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    if serialized.len() <= GEN_AI_MESSAGE_ATTR_LIMIT {
        return serialized;
    }

    for keep_count in GEN_AI_COMPACT_KEEP_COUNTS {
        let compacted = compact_gen_ai_messages(&messages, *keep_count, serialized.len());
        let compacted_serialized = serde_json::to_string(&compacted).unwrap_or_default();
        if compacted_serialized.len() <= GEN_AI_MESSAGE_ATTR_LIMIT {
            return compacted_serialized;
        }
    }

    let fallback = json!([observability_notice_message(format!(
        "LLM input/output payload was too large to attach safely; original payload was {} bytes.",
        serialized.len()
    ))]);
    serde_json::to_string(&fallback).unwrap_or_else(|_| "[]".to_string())
}

fn compact_gen_ai_messages(
    messages: &[Value],
    keep_count: usize,
    original_size: usize,
) -> Vec<Value> {
    if messages.is_empty() {
        return Vec::new();
    }

    let keep_count = keep_count.min(messages.len());
    let omitted = messages.len().saturating_sub(keep_count);
    let mut compacted = Vec::with_capacity(keep_count + usize::from(omitted > 0));

    if omitted > 0 {
        compacted.push(observability_notice_message(format!(
            "{omitted} earlier LLM messages omitted from observability payload; original payload was {original_size} bytes."
        )));
    }

    compacted.extend(
        messages[messages.len() - keep_count..]
            .iter()
            .map(compact_gen_ai_message),
    );

    compacted
}

fn observability_notice_message(content: String) -> Value {
    json!({
        "role": "user",
        "parts": [{
            "type": "text",
            "content": content,
        }],
    })
}

fn compact_gen_ai_message(message: &Value) -> Value {
    let mut compacted = message.clone();
    let Some(parts) = compacted.get_mut("parts").and_then(Value::as_array_mut) else {
        return compacted;
    };

    for part in parts {
        compact_gen_ai_part(part);
    }

    compacted
}

fn compact_gen_ai_part(part: &mut Value) {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return;
    };

    match part_type {
        "text" => {
            if let Some(content) = part.get("content").and_then(Value::as_str) {
                let truncated = truncate_preview(content, GEN_AI_COMPACT_TEXT_LIMIT);
                part["content"] = json!(truncated);
            }
        }
        "tool_call" => {
            if let Some(arguments) = part.get_mut("arguments") {
                *arguments = compact_json_value(arguments, GEN_AI_COMPACT_JSON_LIMIT);
            }
        }
        "tool_call_response" => {
            if let Some(result) = part.get_mut("result") {
                *result = compact_json_value(result, GEN_AI_COMPACT_JSON_LIMIT);
            }
        }
        _ => {}
    }
}

fn compact_json_value(value: &Value, max_bytes: usize) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if serialized.len() <= max_bytes {
        return value.clone();
    }

    json!(truncate_preview(&serialized, max_bytes))
}

fn parts_have_text_content(parts: &[Value]) -> bool {
    parts.iter().any(|part| {
        part.get("type").and_then(Value::as_str) == Some("text")
            && part
                .get("content")
                .and_then(Value::as_str)
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false)
    })
}

fn summarize_tool_call_parts(parts: &[Value]) -> Option<String> {
    let tool_names: Vec<&str> = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_call"))
        .filter_map(|part| part.get("name").and_then(Value::as_str))
        .collect();

    if tool_names.is_empty() {
        return None;
    }

    let visible: Vec<&str> = tool_names.iter().copied().take(8).collect();
    let remaining = tool_names.len().saturating_sub(visible.len());
    let suffix = if remaining == 0 {
        String::new()
    } else {
        format!(" and {remaining} more")
    };

    Some(format!(
        "Requested tool calls: {}{suffix}.",
        visible.join(", ")
    ))
}

fn truncate_for_gen_ai_attr(value: &str) -> String {
    if value.len() <= GEN_AI_MESSAGE_ATTR_LIMIT / 2 {
        return value.to_string();
    }

    let prefix = prefix_at_char_boundary(value, GEN_AI_MESSAGE_ATTR_LIMIT / 4);
    format!("{prefix}... [truncated, {} bytes total]", value.len())
}

fn truncate_preview(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }

    let prefix = prefix_at_char_boundary(value, max_bytes.saturating_sub(64));
    format!("{prefix}... [truncated, {} bytes total]", value.len())
}

fn prefix_at_char_boundary(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    if max_bytes == 0 {
        return "";
    }

    let mut end = 0;
    for (idx, ch) in input.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    &input[..end]
}

fn stringify_content(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        value.to_string()
    }
}
