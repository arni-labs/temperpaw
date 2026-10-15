//! OpenAI Codex Image Generate — provider WASM for MediaGenerationRequest.
//!
//! Triggered by MediaGenerationRequest.RecordAuthReady. Uses the existing Codex
//! subscription OAuth secret flow, calls the ChatGPT/Codex Responses backend
//! with an image_generation tool request, stores the image in PawFS, and
//! records MediaGenerationRequest.RecordResult.

use base64::{Engine as _, engine::general_purpose};
use openai_codex_wire::{
    build_openai_headers, extract_chatgpt_account_id_from_jwt, select_openai_responses_url,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    entity_field_str, resolve_temper_api_url, runtime_headers_as, runtime_headers_for_workspace,
};

const DEFAULT_MODEL: &str = "gpt-5.5";
const DEFAULT_SIZE: &str = "1024x1024";
const DEFAULT_QUALITY: &str = "low";
const DEFAULT_BACKGROUND: &str = "auto";
const DEFAULT_OUTPUT_FORMAT: &str = "png";
const DEFAULT_MEDIA_TYPE: &str = "image";
const DEFAULT_OPERATION: &str = "generate";
const DEFAULT_PROVIDER: &str = "openai_codex";
const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CODEX_RESPONSE_STREAM_CHUNK_BYTES: usize = 256 * 1024;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CODEX_RESPONSE_MAX_BYTES: usize = 64 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
const FILE_UPLOAD_STREAM_CHUNK_BYTES: usize = 256 * 1024;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_openai_codex_image_generate() {
        set_error_result(&err);
    }
    0
}

fn run_openai_codex_image_generate() -> Result<(), String> {
    let ctx = Context::from_host()?;
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

    match generate_and_store(&ctx, &fields) {
        Ok(result) => {
            set_success_result("RecordResult", &record_result_params(&result));
        }
        Err(err) => {
            set_success_result(
                "RecordError",
                &json!({
                    "error": err,
                    "last_error": "openai_codex_image_generate failed",
                }),
            );
        }
    }

    Ok(())
}

struct StoredImageResult {
    file_id: String,
    file_version_id: String,
    path: String,
    mime_type: String,
    revised_prompt: String,
    provider_response_id: String,
    usage_json: String,
}

struct HttpTextResponse {
    status: u16,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexImageOutput {
    base64_data: String,
    partial_base64_data: String,
    mime_type: String,
    response_id: String,
    revised_prompt: String,
    usage_json: String,
}

fn generate_and_store(ctx: &Context, fields: &Value) -> Result<StoredImageResult, String> {
    validate_request(fields)?;
    let prompt = field_or_default(fields, &["prompt", "Prompt"], "");
    if prompt.trim().is_empty() {
        return Err("image_generate: prompt is required".to_string());
    }

    let workspace_id = field_or_default(fields, &["workspace_id", "WorkspaceId"], "");
    if workspace_id.trim().is_empty() {
        return Err("image_generate: workspace_id is required".to_string());
    }

    let model = codex_image_model_or_default(fields, &["model", "Model"], DEFAULT_MODEL);
    let request = build_codex_image_request(fields, prompt, model);
    let access_token = resolve_codex_access_token(ctx)?;
    let account_id = resolve_codex_account_id(ctx, &access_token)?;
    let url = select_codex_image_url(ctx);
    let headers = build_openai_headers("openai_codex", &access_token, Some(&account_id));

    ctx.log(
        "info",
        &format!("openai_codex_image_generate: calling Codex image_generation model={model}"),
    );
    let resp = call_codex_image_generation(ctx, &url, &headers, &request)?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI Codex image generation failed (HTTP {}): {}",
            resp.status,
            sanitized_body_snippet(&resp.body)
        ));
    }

    let output = extract_image_generation_output(&resp.body)?;
    let image_bytes = decode_image_base64(&output.base64_data)?;
    let mime_type = detect_image_mime(&image_bytes)
        .or_else(|| normalize_output_mime(&output.mime_type))
        .unwrap_or_else(|| mime_for_output_format(fields).to_string());
    let output_path = resolve_output_path(fields, ctx, mime_extension(&mime_type));

    record_storing(ctx, fields, &output)?;
    let stored = store_image_file(
        ctx,
        fields,
        workspace_id,
        &output_path,
        &mime_type,
        &image_bytes,
    )?;

    Ok(StoredImageResult {
        file_id: stored.file_id,
        file_version_id: stored.file_version_id,
        path: output_path,
        mime_type,
        revised_prompt: output.revised_prompt,
        provider_response_id: output.response_id,
        usage_json: output.usage_json,
    })
}

fn record_result_params(result: &StoredImageResult) -> Value {
    json!({
        "result_file_id": result.file_id,
        "result_file_version_id": result.file_version_id,
        "result_path": result.path,
        "mime_type": result.mime_type,
        "revised_prompt": result.revised_prompt,
        "provider_response_id": result.provider_response_id,
        "usage_json": result.usage_json,
    })
}

fn validate_request(fields: &Value) -> Result<(), String> {
    let media_type = field_or_default(fields, &["media_type", "MediaType"], DEFAULT_MEDIA_TYPE);
    let operation = field_or_default(fields, &["operation", "Operation"], DEFAULT_OPERATION);
    let provider = normalize_provider(field_or_default(
        fields,
        &["provider", "Provider"],
        DEFAULT_PROVIDER,
    ));

    if !media_type.eq_ignore_ascii_case(DEFAULT_MEDIA_TYPE) {
        return Err(format!("unsupported media_type for v1: {media_type}"));
    }
    if !operation.eq_ignore_ascii_case(DEFAULT_OPERATION) {
        return Err(format!(
            "unsupported media generation operation for v1: {operation}"
        ));
    }
    if provider != DEFAULT_PROVIDER {
        return Err(format!(
            "unsupported media generation provider for v1: {provider}"
        ));
    }
    Ok(())
}

fn normalize_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "" => DEFAULT_PROVIDER.to_string(),
        // "openai" is the guess models make first; both observed hero-image
        // failures (Mullion, Madori 2026-07) burned an attempt on it. There
        // is exactly one v1 provider, so alias every obvious spelling.
        "openai" | "codex" | "openai-codex" | "openai codex" => DEFAULT_PROVIDER.to_string(),
        other => other.to_string(),
    }
}

fn build_codex_image_request(fields: &Value, prompt: &str, model: &str) -> Value {
    let size = field_or_default(fields, &["size", "Size"], DEFAULT_SIZE);
    let quality = field_or_default(fields, &["quality", "Quality"], DEFAULT_QUALITY);
    let background = field_or_default(fields, &["background", "Background"], DEFAULT_BACKGROUND);
    let output_format = normalize_output_format(field_or_default(
        fields,
        &["output_format", "OutputFormat"],
        DEFAULT_OUTPUT_FORMAT,
    ));

    json!({
        "model": model,
        "input": [{ "role": "user", "content": prompt }],
        "instructions": "Generate exactly one image for the user's prompt. Return the generated image through the image_generation tool.",
        "tools": [{
            "type": "image_generation",
            "action": "generate",
            "size": size,
            "quality": quality,
            "background": background,
            "output_format": output_format,
        }],
        "tool_choice": {"type": "image_generation"},
        "stream": true,
        "store": false,
    })
}

fn resolve_codex_access_token(ctx: &Context) -> Result<String, String> {
    let token = first_non_empty_config(ctx, &["openai_codex_access_token", "openai_codex_token"]);
    token.ok_or_else(|| {
        "OpenAI Codex access token is missing; run the Codex subscription auth flow first"
            .to_string()
    })
}

fn resolve_codex_account_id(ctx: &Context, access_token: &str) -> Result<String, String> {
    first_non_empty_config(ctx, &["openai_codex_account_id"])
        .or_else(|| extract_chatgpt_account_id_from_jwt(access_token))
        .ok_or_else(|| {
            "openai_codex requires openai_codex_account_id or a ChatGPT OAuth token containing chatgpt_account_id".to_string()
        })
}

fn first_non_empty_config(ctx: &Context, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        ctx.config
            .get(*key)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
            .map(str::trim)
            .map(ToOwned::to_owned)
    })
}

fn select_codex_image_url(ctx: &Context) -> String {
    let mut config = BTreeMap::new();
    if let Some(value) = ctx.config.get("openai_codex_api_url") {
        config.insert("openai_codex_api_url".to_string(), value.clone());
    }
    let selected = select_openai_responses_url(&config, DEFAULT_PROVIDER);
    if selected.trim().is_empty() {
        CODEX_RESPONSES_URL.to_string()
    } else {
        selected
    }
}

fn extract_image_generation_output(body: &str) -> Result<CodexImageOutput, String> {
    let events = parse_response_events(body);
    let mut output = CodexImageOutput {
        base64_data: String::new(),
        partial_base64_data: String::new(),
        mime_type: String::new(),
        response_id: String::new(),
        revised_prompt: String::new(),
        usage_json: String::new(),
    };

    for event in &events {
        if output.response_id.is_empty() {
            output.response_id = response_id_from_event(event).unwrap_or_default();
        }
        if output.usage_json.is_empty() {
            output.usage_json = usage_json_from_event(event).unwrap_or_default();
        }
        collect_image_generation_call(event, &mut output);
        if !output.base64_data.is_empty() {
            break;
        }
    }
    if output.base64_data.is_empty() {
        collect_image_generation_call_from_raw_body(body, &mut output);
    }

    if output.base64_data.is_empty() {
        if output.partial_base64_data.is_empty() {
            return Err(format!(
                "OpenAI Codex response did not contain an image_generation_call result (body_bytes={}, events={})",
                body.len(),
                summarize_response_events(&events)
            ));
        }
        output.base64_data = std::mem::take(&mut output.partial_base64_data);
    }
    if output.mime_type.is_empty() {
        output.mime_type = "image/png".to_string();
    }
    Ok(output)
}

fn parse_response_events(body: &str) -> Vec<Value> {
    let mut events = Vec::new();
    if let Ok(parsed) = serde_json::from_str::<Value>(body) {
        events.push(parsed);
    }

    let mut current_data = String::new();
    for line in body.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            push_sse_data_event(&mut events, &mut current_data);
            continue;
        }
        if let Some(data) = trimmed.strip_prefix("data:") {
            let data = data.trim_start();
            if data == "[DONE]" {
                push_sse_data_event(&mut events, &mut current_data);
            } else {
                if !current_data.is_empty() {
                    current_data.push('\n');
                }
                current_data.push_str(data);
            }
        }
    }
    push_sse_data_event(&mut events, &mut current_data);

    events
}

fn push_sse_data_event(events: &mut Vec<Value>, current_data: &mut String) {
    let data = current_data.trim();
    if !data.is_empty() {
        if let Ok(parsed) = serde_json::from_str::<Value>(data) {
            events.push(parsed);
        }
    }
    current_data.clear();
}

fn collect_image_generation_call(value: &Value, output: &mut CodexImageOutput) {
    match value {
        Value::Object(obj) => {
            if obj.get("type").and_then(Value::as_str) == Some("image_generation_call") {
                apply_image_generation_object(value, output);
            }
            if obj.get("type").and_then(Value::as_str)
                == Some("response.image_generation_call.partial_image")
            {
                apply_partial_image_generation_object(value, output);
            }
            if let Some(item) = obj.get("item") {
                collect_image_generation_call(item, output);
            }
            if let Some(response) = obj.get("response") {
                collect_image_generation_call(response, output);
            }
            if let Some(output_items) = obj.get("output") {
                collect_image_generation_call(output_items, output);
            }
            if let Some(content) = obj.get("content") {
                collect_image_generation_call(content, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_image_generation_call(item, output);
                if !output.base64_data.is_empty() {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn collect_image_generation_call_from_raw_body(body: &str, output: &mut CodexImageOutput) {
    if output.response_id.is_empty() {
        output.response_id =
            extract_json_string_field_with_prefix(body, "id", "resp_").unwrap_or_default();
    }
    if body.contains("image_generation_call") && output.base64_data.is_empty() {
        output.base64_data = extract_json_string_field(body, "result").unwrap_or_default();
    }
    if output.partial_base64_data.is_empty() {
        output.partial_base64_data =
            extract_json_string_field(body, "partial_image_b64").unwrap_or_default();
    }
    if output.mime_type.is_empty() {
        output.mime_type = extract_json_string_field(body, "mime_type")
            .or_else(|| extract_json_string_field(body, "media_type"))
            .or_else(|| extract_json_string_field(body, "output_format"))
            .as_deref()
            .and_then(normalize_output_mime)
            .unwrap_or_default();
    }
    if output.revised_prompt.is_empty() {
        output.revised_prompt =
            extract_json_string_field(body, "revised_prompt").unwrap_or_default();
    }
}

fn extract_json_string_field_with_prefix(body: &str, field: &str, prefix: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let mut search_start = 0;
    while let Some(relative_pos) = body[search_start..].find(&needle) {
        let pos = search_start + relative_pos + needle.len();
        let after_key = &body[pos..];
        let Some(colon_pos) = after_key.find(':') else {
            return None;
        };
        let after_colon = after_key[colon_pos + 1..].trim_start();
        if let Some(rest) = after_colon.strip_prefix('"') {
            if let Some(value) = parse_json_string_contents(rest) {
                if value.starts_with(prefix) {
                    return Some(value);
                }
            }
        }
        search_start = pos;
    }
    None
}

fn extract_json_string_field(body: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let mut search_start = 0;
    while let Some(relative_pos) = body[search_start..].find(&needle) {
        let pos = search_start + relative_pos + needle.len();
        let after_key = &body[pos..];
        let colon_pos = after_key.find(':')?;
        let after_colon = after_key[colon_pos + 1..].trim_start();
        if let Some(rest) = after_colon.strip_prefix('"') {
            return parse_json_string_contents(rest);
        }
        search_start = pos;
    }
    None
}

fn parse_json_string_contents(rest: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let code = u16::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code as u32)?);
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

fn apply_partial_image_generation_object(value: &Value, output: &mut CodexImageOutput) {
    if output.partial_base64_data.is_empty() {
        output.partial_base64_data = value
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
    }
    if output.mime_type.is_empty() {
        output.mime_type = value
            .get("mime_type")
            .or_else(|| value.get("media_type"))
            .or_else(|| value.get("output_format"))
            .and_then(Value::as_str)
            .and_then(normalize_output_mime)
            .unwrap_or_default();
    }
}

fn apply_image_generation_object(value: &Value, output: &mut CodexImageOutput) {
    if output.base64_data.is_empty() {
        output.base64_data = value
            .get("result")
            .or_else(|| value.get("image_base64"))
            .or_else(|| value.get("b64_json"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
    }
    if output.mime_type.is_empty() {
        output.mime_type = value
            .get("mime_type")
            .or_else(|| value.get("media_type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
    if output.revised_prompt.is_empty() {
        output.revised_prompt = value
            .get("revised_prompt")
            .or_else(|| value.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
    }
}

fn summarize_response_events(events: &[Value]) -> String {
    events
        .iter()
        .take(12)
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(",")
}

fn response_id_from_event(value: &Value) -> Option<String> {
    value
        .get("response")
        .and_then(|response| response.get("id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn usage_json_from_event(value: &Value) -> Option<String> {
    let usage = value
        .get("response")
        .and_then(|response| response.get("usage"))
        .or_else(|| value.get("usage"))?;
    serde_json::to_string(usage).ok()
}

fn decode_image_base64(base64_data: &str) -> Result<Vec<u8>, String> {
    let compact: String = base64_data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|err| format!("OpenAI Codex returned invalid image base64: {err}"))
}

fn detect_image_mime(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png".to_string())
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg".to_string())
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp".to_string())
    } else {
        None
    }
}

fn normalize_output_mime(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "image/png" | "png" => Some("image/png".to_string()),
        "image/jpeg" | "image/jpg" | "jpeg" | "jpg" => Some("image/jpeg".to_string()),
        "image/webp" | "webp" => Some("image/webp".to_string()),
        _ => None,
    }
}

fn normalize_output_format(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "image/png" | "png" | "" => "png".to_string(),
        "image/jpeg" | "image/jpg" | "jpeg" | "jpg" => "jpeg".to_string(),
        "image/webp" | "webp" => "webp".to_string(),
        other => other.to_string(),
    }
}

fn mime_for_output_format(fields: &Value) -> &'static str {
    match normalize_output_format(field_or_default(
        fields,
        &["output_format", "OutputFormat"],
        DEFAULT_OUTPUT_FORMAT,
    ))
    .as_str()
    {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn mime_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn resolve_output_path(fields: &Value, ctx: &Context, ext: &str) -> String {
    let configured = field_or_default(fields, &["output_path", "OutputPath"], "");
    if !configured.trim().is_empty() {
        return ensure_path_extension(configured.trim(), ext);
    }
    let id = ctx
        .entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(ctx.entity_id.as_str());
    format!("/generated/images/{id}.{ext}")
}

fn ensure_path_extension(path: &str, ext: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
    {
        path.to_string()
    } else {
        format!("{path}.{ext}")
    }
}

struct StoredFile {
    file_id: String,
    file_version_id: String,
}

fn store_image_file(
    ctx: &Context,
    fields: &Value,
    workspace_id: &str,
    path: &str,
    mime_type: &str,
    bytes: &[u8],
) -> Result<StoredFile, String> {
    let temper_api_url = resolve_temper_api_url(ctx, fields);
    let file_name = path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("generated-image.png");
    let file_body = json!({
        "Name": file_name,
        "Path": path,
        "WorkspaceId": workspace_id,
        "MimeType": mime_type,
    });
    let headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        Some("application/json"),
        Some("application/json"),
    );
    let create_url = format!("{temper_api_url}/tdata/Files");
    let create_resp = ctx.http_call("POST", &create_url, &headers, &file_body.to_string())?;
    if !(200..300).contains(&create_resp.status) {
        return Err(format!(
            "image_generate: PawFS File create failed (HTTP {}): {}",
            create_resp.status,
            sanitized_body_snippet(&create_resp.body)
        ));
    }
    let file_value: Value = serde_json::from_str(&create_resp.body)
        .map_err(|err| format!("image_generate: parse PawFS File create response: {err}"))?;
    let file_id = entity_field_str(&file_value, &["Id", "id"])
        .or_else(|| file_value.get("entity_id").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or("image_generate: PawFS File create response did not include an id")?
        .to_string();

    let value_url = format!("{temper_api_url}/tdata/Files('{file_id}')/$value");
    let value_headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        Some(mime_type),
        None,
    );
    put_file_value_stream(&value_url, &value_headers, bytes)?;

    let head_headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        None,
        Some("application/json"),
    );
    let head_url = format!("{temper_api_url}/tdata/Files('{file_id}')");
    let head_resp = ctx.http_call("GET", &head_url, &head_headers, "")?;
    if !(200..300).contains(&head_resp.status) {
        return Err(format!(
            "image_generate: PawFS File read-after-write failed (HTTP {}): {}",
            head_resp.status,
            sanitized_body_snippet(&head_resp.body)
        ));
    }
    let head_value: Value = serde_json::from_str(&head_resp.body)
        .map_err(|err| format!("image_generate: parse PawFS File head response: {err}"))?;
    let file_version_id = entity_field_str(&head_value, &["LastVersionId", "last_version_id"])
        .unwrap_or("")
        .to_string();

    Ok(StoredFile {
        file_id,
        file_version_id,
    })
}

fn record_storing(ctx: &Context, fields: &Value, output: &CodexImageOutput) -> Result<(), String> {
    let temper_api_url = resolve_temper_api_url(ctx, fields);
    let entity_id = ctx
        .entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(ctx.entity_id.as_str());
    let url = format!(
        "{temper_api_url}/tdata/MediaGenerationRequests('{}')/Temper.RecordStoring",
        escape_odata_key(entity_id)
    );
    let headers = runtime_headers_as(
        ctx,
        &ctx.tenant,
        fields,
        "system",
        Some("application/json"),
        Some("application/json"),
    );
    let body = json!({
        "provider_response_id": output.response_id,
        "revised_prompt": output.revised_prompt,
        "usage_json": output.usage_json,
    });
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        ctx.log(
            "warn",
            &format!(
                "openai_codex_image_generate: RecordStoring failed (HTTP {}): {}",
                resp.status,
                sanitized_body_snippet(&resp.body)
            ),
        );
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn call_codex_image_generation(
    _ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    request: &Value,
) -> Result<HttpTextResponse, String> {
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (mut request_body, mut response_body, response_head) =
        temper_wasm_sdk::http_stream::streaming_call("POST", url, &header_refs)
            .map_err(|error| format!("OpenAI Codex streaming request failed to start: {error}"))?;

    let body = request.to_string();
    for chunk in body.as_bytes().chunks(CODEX_RESPONSE_STREAM_CHUNK_BYTES) {
        request_body
            .write_all_chunk(chunk)
            .map_err(|error| format!("OpenAI Codex streaming request write failed: {error}"))?;
    }
    request_body
        .finish()
        .map_err(|error| format!("OpenAI Codex streaming request close failed: {error}"))?;

    let head = response_head()
        .map_err(|error| format!("OpenAI Codex streaming response head failed: {error}"))?;
    let mut body_bytes = Vec::new();
    let mut buffer = vec![0u8; CODEX_RESPONSE_STREAM_CHUNK_BYTES];
    loop {
        let Some(read) = response_body
            .read_next_chunk(&mut buffer)
            .map_err(|error| format!("OpenAI Codex streaming response read failed: {error}"))?
        else {
            break;
        };
        body_bytes.extend_from_slice(&buffer[..read]);
        if body_bytes.len() > CODEX_RESPONSE_MAX_BYTES {
            let _ = response_body.close();
            return Err(format!(
                "OpenAI Codex image generation response exceeded {} bytes",
                CODEX_RESPONSE_MAX_BYTES
            ));
        }
    }
    response_body
        .close()
        .map_err(|error| format!("OpenAI Codex streaming response close failed: {error}"))?;
    let body = String::from_utf8(body_bytes)
        .map_err(|error| format!("OpenAI Codex streaming response was not UTF-8: {error}"))?;
    Ok(HttpTextResponse {
        status: head.status,
        body,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn call_codex_image_generation(
    ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    request: &Value,
) -> Result<HttpTextResponse, String> {
    let resp = ctx.http_call("POST", url, headers, &request.to_string())?;
    Ok(HttpTextResponse {
        status: resp.status,
        body: resp.body,
    })
}

#[cfg(target_arch = "wasm32")]
fn put_file_value_stream(
    url: &str,
    headers: &[(String, String)],
    bytes: &[u8],
) -> Result<(), String> {
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (mut request_body, response_body, response_head) =
        temper_wasm_sdk::http_stream::streaming_call("PUT", url, &header_refs)
            .map_err(|error| format!("streaming PawFS image upload failed to start: {error}"))?;

    for chunk in bytes.chunks(FILE_UPLOAD_STREAM_CHUNK_BYTES) {
        request_body.write_all_chunk(chunk).map_err(|error| {
            format!("streaming PawFS image upload failed while writing body: {error}")
        })?;
    }
    request_body.finish().map_err(|error| {
        format!("streaming PawFS image upload failed while closing body: {error}")
    })?;

    let head = response_head()
        .map_err(|error| format!("streaming PawFS image upload failed before response: {error}"))?;
    let _ = response_body.close();
    if head.status >= 400 || head.status == 0 {
        let stream_error = head
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("x-temper-stream-error"))
            .map(|(_, value)| format!(": {value}"))
            .unwrap_or_default();
        return Err(format!(
            "PawFS image upload failed (HTTP {}{stream_error})",
            head.status
        ));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn put_file_value_stream(
    _url: &str,
    _headers: &[(String, String)],
    _bytes: &[u8],
) -> Result<(), String> {
    Err("streaming PawFS image uploads require the Temper WASM host".to_string())
}

fn field_or_default<'a>(value: &'a Value, keys: &[&str], default: &'a str) -> &'a str {
    entity_field_str(value, keys)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn codex_image_model_or_default<'a>(value: &'a Value, keys: &[&str], default: &'a str) -> &'a str {
    let model = field_or_default(value, keys, default);
    if is_public_openai_image_model_name(model) {
        default
    } else {
        model
    }
}

fn is_public_openai_image_model_name(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized == "gpt-image"
        || normalized.starts_with("gpt-image-")
        || normalized == "dall-e"
        || normalized.starts_with("dall-e-")
}

fn escape_odata_key(key: &str) -> String {
    key.replace('\'', "''")
}

fn sanitized_body_snippet(body: &str) -> String {
    let mut snippet = String::new();
    for ch in body.chars().take(500) {
        snippet.push(if ch.is_control() && ch != '\n' && ch != '\t' {
            ' '
        } else {
            ch
        });
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=";

    #[test]
    fn build_codex_image_request_uses_responses_image_generation_tool() {
        let fields = json!({
            "size": "1536x1024",
            "quality": "high",
            "output_format": "image/png",
            "background": "opaque"
        });

        let request = build_codex_image_request(&fields, "paint a quiet lighthouse", "gpt-5.5");

        assert_eq!(request["model"], "gpt-5.5");
        assert_eq!(
            request["input"],
            json!([{ "role": "user", "content": "paint a quiet lighthouse" }])
        );
        assert_eq!(request["stream"], true);
        assert_eq!(request["tools"][0]["type"], "image_generation");
        assert_eq!(request["tools"][0]["action"], "generate");
        assert_eq!(request["tools"][0]["size"], "1536x1024");
        assert_eq!(request["tools"][0]["quality"], "high");
        assert_eq!(request["tools"][0]["output_format"], "png");
    }

    #[test]
    fn default_image_quality_stays_within_temper_http_buffer() {
        let request = build_codex_image_request(&json!({}), "paint a quiet lighthouse", "gpt-5.5");

        assert_eq!(request["tools"][0]["quality"], "low");
    }

    #[test]
    fn empty_model_field_uses_provider_default() {
        let fields = json!({
            "Model": "",
        });

        assert_eq!(
            field_or_default(&fields, &["model", "Model"], DEFAULT_MODEL),
            DEFAULT_MODEL
        );
    }

    #[test]
    fn public_openai_image_model_names_use_codex_default() {
        let fields = json!({
            "Model": "gpt-image-2",
        });

        assert_eq!(
            codex_image_model_or_default(&fields, &["model", "Model"], DEFAULT_MODEL),
            DEFAULT_MODEL
        );
    }

    #[test]
    fn record_result_params_omit_inline_base64_payload() {
        let result = StoredImageResult {
            file_id: "fl-cat".to_string(),
            file_version_id: "fv-cat".to_string(),
            path: "/generated/cat.png".to_string(),
            mime_type: "image/png".to_string(),
            revised_prompt: "cat in a window".to_string(),
            provider_response_id: "resp_cat".to_string(),
            usage_json: "{}".to_string(),
        };

        let params = record_result_params(&result);

        assert_eq!(params["result_file_id"], "fl-cat");
        assert_eq!(params["result_path"], "/generated/cat.png");
        assert!(params.get("result_image_base64").is_none());
    }

    #[test]
    fn parse_streamed_image_generation_call_extracts_base64_result() {
        let body = format!(
            "event: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_123",
                    "usage": {"input_tokens": 10, "output_tokens": 5},
                    "output": [{
                        "type": "image_generation_call",
                        "result": PNG_1X1_BASE64,
                        "mime_type": "image/png",
                        "revised_prompt": "A quiet lighthouse at dawn."
                    }]
                }
            })
        );

        let output = extract_image_generation_output(&body).unwrap();

        assert_eq!(output.base64_data, PNG_1X1_BASE64);
        assert_eq!(output.mime_type, "image/png");
        assert_eq!(output.response_id, "resp_123");
        assert_eq!(output.revised_prompt, "A quiet lighthouse at dawn.");
        assert!(output.usage_json.contains("input_tokens"));
    }

    #[test]
    fn parse_output_item_done_image_generation_call_extracts_base64_result() {
        let body = format!(
            "data: {}\n\n",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "image_generation_call",
                    "result": PNG_1X1_BASE64,
                    "media_type": "image/png"
                }
            })
        );

        let output = extract_image_generation_output(&body).unwrap();

        assert_eq!(output.base64_data, PNG_1X1_BASE64);
        assert_eq!(output.mime_type, "image/png");
    }

    #[test]
    fn parse_partial_image_generation_call_as_fallback() {
        let body = format!(
            "data: {}\n\ndata: {}\n\n",
            json!({
                "type": "response.image_generation_call.partial_image",
                "partial_image_b64": PNG_1X1_BASE64,
                "output_format": "png",
                "partial_image_index": 0
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "status": "completed"
                }
            })
        );

        let output = extract_image_generation_output(&body).unwrap();

        assert_eq!(output.base64_data, PNG_1X1_BASE64);
        assert_eq!(output.mime_type, "image/png");
        assert_eq!(output.response_id, "resp_1");
    }

    #[test]
    fn parse_raw_buffered_image_generation_result_without_sse_data_prefixes() {
        let body = format!(
            "event: response.output_item.done\n{}\nevent: response.completed\n{}",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "id": "ig_1",
                    "type": "image_generation_call",
                    "result": PNG_1X1_BASE64,
                    "output_format": "png"
                }
            }),
            json!({
                "type": "response.completed",
                "response": {"id": "resp_1"}
            })
        );

        let output = extract_image_generation_output(&body).unwrap();

        assert_eq!(output.base64_data, PNG_1X1_BASE64);
        assert_eq!(output.mime_type, "image/png");
        assert_eq!(output.response_id, "resp_1");
    }

    #[test]
    fn output_format_and_mime_are_normalized() {
        assert_eq!(normalize_output_format("image/jpeg"), "jpeg");
        assert_eq!(normalize_output_format("jpg"), "jpeg");
        assert_eq!(normalize_output_mime("webp").as_deref(), Some("image/webp"));
        assert_eq!(mime_extension("image/jpeg"), "jpg");
    }

    #[test]
    fn detects_png_bytes() {
        let bytes = decode_image_base64(PNG_1X1_BASE64).unwrap();
        assert_eq!(detect_image_mime(&bytes).as_deref(), Some("image/png"));
    }

    #[test]
    fn output_path_gets_image_extension() {
        let fields = json!({"output_path": "/generated/custom"});
        let ctx = Context {
            tenant: "default".to_string(),
            entity_type: "MediaGenerationRequest".to_string(),
            entity_id: "mg-1".to_string(),
            trigger_params: json!({}),
            trigger_action: "RecordAuthReady".to_string(),
            wasm_module: "openai_codex_image_generate".to_string(),
            entity_state: json!({"entity_id": "mg-1", "fields": {}}),
            config: BTreeMap::new(),
            http_request: None,
        };

        assert_eq!(
            resolve_output_path(&fields, &ctx, "png"),
            "/generated/custom.png"
        );
    }
}
