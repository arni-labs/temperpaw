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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_ai_system_instructions_serializes_text_content() {
        let payload = build_gen_ai_system_instructions("You are a precise assistant.");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed[0]["type"], "text");
        assert_eq!(parsed[0]["content"], "You are a precise assistant.");
    }

    #[test]
    fn gen_ai_input_messages_preserve_chat_history_and_tool_results() {
        let payload = build_gen_ai_input_messages(&[
            json!({"role": "user", "content": "List the recent sessions."}),
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "tool_123",
                    "name": "temper.list_sessions",
                    "input": {"top": 3}
                }]
            }),
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "tool_123",
                    "content": [{"type": "text", "text": "[\"s1\",\"s2\"]"}],
                    "is_error": false
                }]
            }),
        ]);

        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed[0]["role"], "user");
        assert_eq!(parsed[0]["parts"][0]["type"], "text");
        assert_eq!(parsed[1]["role"], "assistant");
        assert_eq!(parsed[1]["parts"][0]["type"], "tool_call");
        assert_eq!(parsed[1]["parts"][0]["name"], "temper.list_sessions");
        assert_eq!(parsed[2]["role"], "tool");
        assert_eq!(parsed[2]["parts"][0]["type"], "tool_call_response");
        assert_eq!(parsed[2]["parts"][0]["id"], "tool_123");
    }

    #[test]
    fn gen_ai_output_messages_preserve_text_and_tool_calls() {
        let payload = build_gen_ai_output_messages(
            &json!([
                {"type": "text", "text": "I need to inspect the latest sessions first."},
                {
                    "type": "tool_use",
                    "id": "tool_456",
                    "name": "temper.list_sessions",
                    "input": {"top": 5}
                }
            ]),
            "tool_use",
        );

        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed[0]["role"], "assistant");
        assert_eq!(parsed[0]["finish_reason"], "tool_use");
        assert_eq!(parsed[0]["parts"][0]["type"], "text");
        assert_eq!(parsed[0]["parts"][1]["type"], "tool_call");
        assert_eq!(parsed[0]["parts"][1]["id"], "tool_456");
    }

    #[test]
    fn gen_ai_input_messages_compact_large_payload_without_erasing_recent_context() {
        let huge_tool_result = "x".repeat(80_000);
        let payload = build_gen_ai_input_messages(&[
            json!({"role": "user", "content": "Start the curation repair job."}),
            json!({
                "role": "tool",
                "tool_use_id": "tool_huge",
                "content": huge_tool_result,
            }),
            json!({"role": "user", "content": "Now summarize the repair result for Datadog."}),
        ]);

        assert!(payload.len() <= GEN_AI_MESSAGE_ATTR_LIMIT);
        assert!(
            !payload.contains("\"content\":\"[truncated,"),
            "payload should keep compact real messages rather than a single placeholder: {payload}"
        );

        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
        assert_eq!(
            parsed[2]["parts"][0]["content"],
            "Now summarize the repair result for Datadog."
        );
        assert_eq!(parsed[1]["parts"][0]["type"], "tool_call_response");
        assert!(
            parsed[1]["parts"][0]["result"]
                .as_str()
                .unwrap()
                .contains("truncated")
        );
    }

    #[test]
    fn gen_ai_output_messages_add_summary_for_tool_only_responses() {
        let payload = build_gen_ai_output_messages(
            &json!([
                {
                    "type": "tool_use",
                    "id": "tool_456",
                    "name": "temper.list_sessions",
                    "input": {"top": 5}
                }
            ]),
            "tool_use",
        );

        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed[0]["role"], "assistant");
        assert_eq!(parsed[0]["parts"][0]["type"], "text");
        assert_eq!(
            parsed[0]["parts"][0]["content"],
            "Requested tool calls: temper.list_sessions."
        );
        assert_eq!(parsed[0]["parts"][1]["type"], "tool_call");
        assert_eq!(parsed[0]["parts"][1]["id"], "tool_456");
    }

    #[test]
    fn provider_response_ready_params_include_llm_observability_content() {
        let prepared = PreparedContextArtifact {
            version: 1,
            messages: vec![json!({"role": "user", "content": "What changed?"})],
            tools: vec![],
            system_prompt: "You are concise.".to_string(),
            system_prompt_hash: "hash-123".to_string(),
            system_prompt_file_id: "file-system".to_string(),
            conversation_file_id: String::new(),
            session_file_id: String::new(),
            session_leaf_id: String::new(),
            workspace_id: "workspace-1".to_string(),
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
            content: json!([{"type": "text", "text": "The LLM span now has content."}]),
            stop_reason: "end_turn".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 256,
            response_bytes: 512,
            token_signals: None,
        };

        let params =
            build_provider_response_ready_params("provider-response-file", &prepared, &artifact);

        assert_eq!(
            params["provider_response_file_id"],
            "provider-response-file"
        );
        assert_eq!(params["_gen_ai_provider"], "anthropic");
        assert_eq!(params["_gen_ai_model"], "claude-sonnet-4-6");
        assert_eq!(params["_gen_ai_finish_reason"], "end_turn");

        let input: Value =
            serde_json::from_str(params["_gen_ai_input_messages"].as_str().unwrap()).unwrap();
        let output: Value =
            serde_json::from_str(params["_gen_ai_output_messages"].as_str().unwrap()).unwrap();
        let system: Value =
            serde_json::from_str(params["_gen_ai_system_instructions"].as_str().unwrap()).unwrap();

        assert_eq!(system[0]["content"], "You are concise.");
        assert_eq!(input[0]["parts"][0]["content"], "What changed?");
        assert_eq!(
            output[0]["parts"][0]["content"],
            "The LLM span now has content."
        );
    }

    #[test]
    fn prepared_context_artifact_defaults_prune_window_for_legacy_files() {
        let artifact: PreparedContextArtifact = serde_json::from_value(json!({
            "version": 1,
            "messages": [],
            "tools": [],
            "system_prompt": "",
            "system_prompt_hash": "",
            "system_prompt_file_id": "",
            "conversation_file_id": "",
            "session_file_id": "session-1",
            "session_leaf_id": "u-1",
            "workspace_id": "workspace-1",
            "use_session_tree": true,
            "context_tokens": 0,
            "context_bytes": 0,
            "entries_loaded": 0,
            "content_files_loaded": 0
        }))
        .expect("legacy prepared context artifact should deserialize");

        assert_eq!(artifact.prune_tool_results_after_turns, 4);
    }

    #[test]
    fn parses_prepared_context_artifact_from_direct_or_json_string_payload() {
        let raw = json!({
            "version": 1,
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [],
            "system_prompt": "You are concise.",
            "system_prompt_hash": "hash-123",
            "system_prompt_file_id": "system-file",
            "conversation_file_id": "",
            "session_file_id": "session-entries:session-1",
            "session_leaf_id": "u-1",
            "workspace_id": "workspace-1",
            "use_session_tree": true,
            "context_tokens": 12,
            "context_bytes": 128,
            "entries_loaded": 1,
            "content_files_loaded": 0
        })
        .to_string();
        let wrapped = serde_json::to_string(&raw).unwrap();

        let direct = parse_prepared_context_artifact(&raw).unwrap();
        let encoded = parse_prepared_context_artifact(&wrapped).unwrap();

        assert_eq!(direct.session_leaf_id, "u-1");
        assert_eq!(encoded.session_leaf_id, "u-1");
        assert_eq!(encoded.prune_tool_results_after_turns, 4);
    }

    #[test]
    fn parses_provider_response_artifact_from_direct_or_json_string_payload() {
        let raw = json!({
            "version": 1,
            "provider": "mock",
            "model": "mock-fast",
            "content": [{"type": "text", "text": "hi"}],
            "stop_reason": "end_turn",
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_read_input_tokens": 0,
            "cache_creation_input_tokens": 0,
            "request_bytes": 64,
            "response_bytes": 32
        })
        .to_string();
        let wrapped = serde_json::to_string(&raw).unwrap();

        let direct = parse_provider_response_artifact(&raw).unwrap();
        let encoded = parse_provider_response_artifact(&wrapped).unwrap();

        assert_eq!(direct.stop_reason, "end_turn");
        assert_eq!(encoded.provider, "mock");
    }

    #[test]
    fn provider_response_applier_base_params_do_not_emit_llm_observability_content() {
        let prepared = PreparedContextArtifact {
            version: 1,
            messages: vec![json!({"role": "user", "content": "What changed?"})],
            tools: vec![],
            system_prompt: "You are concise.".to_string(),
            system_prompt_hash: "hash-123".to_string(),
            system_prompt_file_id: "file-system".to_string(),
            conversation_file_id: String::new(),
            session_file_id: String::new(),
            session_leaf_id: String::new(),
            workspace_id: "workspace-1".to_string(),
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
            content: json!([{"type": "text", "text": "The provider call already emitted LLMObs content."}]),
            stop_reason: "end_turn".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            request_bytes: 256,
            response_bytes: 512,
            token_signals: None,
        };

        let params = build_provider_response_applier_base_params(&prepared, &artifact);

        assert_eq!(params["input_tokens"], 10);
        assert_eq!(params["output_tokens"], 20);
        assert_eq!(params["system_prompt_hash"], "hash-123");
        assert!(params.get("_gen_ai_system_instructions").is_none());
        assert!(params.get("_gen_ai_input_messages").is_none());
        assert!(params.get("_gen_ai_output_messages").is_none());
        assert!(params.get("_gen_ai_provider").is_none());
        assert!(params.get("_gen_ai_model").is_none());
        assert!(params.get("_gen_ai_finish_reason").is_none());
    }
}
