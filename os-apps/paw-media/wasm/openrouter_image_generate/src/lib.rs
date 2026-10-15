//! OpenRouter Image Generate — second provider WASM for MediaGenerationRequest.
//!
//! Triggered by MediaGenerationRequest.GenerateWithOpenRouter. Calls
//! OpenRouter's image API (`POST /api/v1/images`) with an API key held as a
//! Temper secret, stores the image in PawFS, and records
//! MediaGenerationRequest.RecordResult. No subscription auth gate: the key is
//! the credential. The default model is xAI's Grok Imagine; Katagami's
//! art-style transfer test also names GPT Image, Nano Banana and Seedream
//! (ALLOWED_MODELS).

use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{
    entity_field_str, resolve_temper_api_url, runtime_headers_as, runtime_headers_for_workspace,
};

const DEFAULT_MODEL: &str = "x-ai/grok-imagine-image-2.0";
const DEFAULT_MEDIA_TYPE: &str = "image";
const DEFAULT_OPERATION: &str = "generate";
const OPENROUTER_IMAGES_URL: &str = "https://openrouter.ai/api/v1/images";
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const RESPONSE_STREAM_CHUNK_BYTES: usize = 256 * 1024;
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const RESPONSE_MAX_BYTES: usize = 64 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
const FILE_UPLOAD_STREAM_CHUNK_BYTES: usize = 256 * 1024;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_openrouter_image_generate() {
        set_error_result(&err);
    }
    0
}

fn run_openrouter_image_generate() -> Result<(), String> {
    let ctx = Context::from_host()?;
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

    match generate_and_store(&ctx, &fields) {
        Ok(result) => set_success_result("RecordResult", &record_result_params(&result)),
        Err(err) => set_success_result(
            "RecordError",
            &json!({
                "error": err,
                "last_error": "openrouter_image_generate failed",
            }),
        ),
    }
    Ok(())
}

struct StoredImageResult {
    file_id: String,
    file_version_id: String,
    path: String,
    mime_type: String,
    model: String,
    usage_json: String,
}

struct HttpTextResponse {
    status: u16,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageOutput {
    base64_data: String,
    media_type: String,
    usage_json: String,
}

fn generate_and_store(ctx: &Context, fields: &Value) -> Result<StoredImageResult, String> {
    validate_request(fields)?;
    let prompt = field_or_default(fields, &["prompt", "Prompt"], "");
    if prompt.is_empty() {
        return Err("image_generate: prompt is required".to_string());
    }
    let workspace_id = field_or_default(fields, &["workspace_id", "WorkspaceId"], "");
    if workspace_id.is_empty() {
        return Err("image_generate: workspace_id is required".to_string());
    }

    let model = openrouter_model(fields, ctx)?;
    let request = build_openrouter_image_request(fields, prompt, &model);
    let api_key = config_value(ctx, "openrouter_api_key").ok_or(
        "OpenRouter API key is missing: set the tenant secret openrouter_api_key".to_string(),
    )?;
    let url = config_value(ctx, "openrouter_images_url")
        .unwrap_or_else(|| OPENROUTER_IMAGES_URL.to_string());
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {api_key}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("X-Title".to_string(), "TemperPaw media".to_string()),
    ];

    // The File is created before the paid call: if this identity may not write
    // there (a policy or workspace problem), the request fails before any
    // OpenRouter credit is spent. The bytes are uploaded once they exist.
    let planned_mime = requested_mime(fields);
    let output_path = resolve_output_path(fields, ctx, mime_extension(&planned_mime));
    let file_id = create_image_file(ctx, fields, workspace_id, &output_path, &planned_mime)?;

    ctx.log(
        "info",
        &format!("openrouter_image_generate: calling OpenRouter images model={model}"),
    );
    let resp = call_openrouter(ctx, &url, &headers, &request)?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenRouter image generation failed (HTTP {}): {}",
            resp.status,
            sanitized_body_snippet(&resp.body)
        ));
    }

    let output = extract_image_output(&resp.body)?;
    let image_bytes = decode_image_base64(&output.base64_data)?;
    let mime_type = detect_image_mime(&image_bytes)
        .or_else(|| normalize_output_mime(&output.media_type))
        .unwrap_or_else(|| planned_mime.clone());

    record_storing(ctx, fields, &model, &output)?;
    let file_version_id = upload_image_bytes(
        ctx,
        fields,
        workspace_id,
        &file_id,
        &mime_type,
        &image_bytes,
    )?;

    Ok(StoredImageResult {
        file_id,
        file_version_id,
        path: output_path,
        mime_type,
        model,
        usage_json: output.usage_json,
    })
}

fn record_result_params(result: &StoredImageResult) -> Value {
    json!({
        "result_file_id": result.file_id,
        "result_file_version_id": result.file_version_id,
        "result_path": result.path,
        "mime_type": result.mime_type,
        // OpenRouter returns no response id or revised prompt; the model that
        // drew it is recorded where the Codex renderer records its response id.
        "provider_response_id": format!("openrouter:{}", result.model),
        "revised_prompt": "",
        "usage_json": result.usage_json,
    })
}

/// The action chose the provider; only the media type and operation are checked.
fn validate_request(fields: &Value) -> Result<(), String> {
    let media_type = field_or_default(fields, &["media_type", "MediaType"], DEFAULT_MEDIA_TYPE);
    let operation = field_or_default(fields, &["operation", "Operation"], DEFAULT_OPERATION);
    if !media_type.eq_ignore_ascii_case(DEFAULT_MEDIA_TYPE) {
        return Err(format!(
            "unsupported media_type for OpenRouter: {media_type}"
        ));
    }
    if !operation.eq_ignore_ascii_case(DEFAULT_OPERATION) {
        return Err(format!(
            "unsupported media generation operation for OpenRouter: {operation}"
        ));
    }
    Ok(())
}

/// OpenRouter models a request may name. Every picture is paid from the
/// tenant's OpenRouter credit, so a caller cannot pick an arbitrary (or
/// arbitrarily expensive) model: only the newest model of each family
/// Katagami's art-style transfer test draws with (Rita, 2026-10-02: Grok
/// Imagine, GPT Image, Nano Banana and Seedream, ids from OpenRouter's image
/// model list on 2026-10-02), or the operator's configured default.
const ALLOWED_MODELS: &[&str] = &[
    DEFAULT_MODEL,
    "openai/gpt-image-2.5-sunburst",
    "google/gemini-3.1-flash-image",
    "bytedance-seed/seedream-5-0-pro",
];

/// The configured default (else Grok Imagine), or a requested "vendor/model"
/// when it is allowed.
fn openrouter_model(fields: &Value, ctx: &Context) -> Result<String, String> {
    let configured =
        config_value(ctx, "default_model").unwrap_or_else(|| DEFAULT_MODEL.to_string());
    choose_model(
        field_or_default(fields, &["model", "Model"], ""),
        &configured,
    )
}

fn choose_model(asked: &str, configured: &str) -> Result<String, String> {
    if !asked.contains('/') || asked == configured {
        return Ok(configured.to_string());
    }
    if ALLOWED_MODELS.contains(&asked) {
        return Ok(asked.to_string());
    }
    Err(format!(
        "OpenRouter model {asked} is not allowed here; use {configured} (or leave model empty)"
    ))
}

/// Only what every OpenRouter image model takes: model, prompt and one image.
/// A non-square size becomes an aspect ratio; quality and background are not
/// sent, because the endpoint rejects a parameter a model does not support.
fn build_openrouter_image_request(fields: &Value, prompt: &str, model: &str) -> Value {
    let mut request = json!({ "model": model, "prompt": prompt, "n": 1 });
    if let Some(ratio) = aspect_ratio_for_size(field_or_default(fields, &["size", "Size"], "")) {
        request["aspect_ratio"] = json!(ratio);
    }
    request
}

fn aspect_ratio_for_size(size: &str) -> Option<&'static str> {
    let (w, h) = size.trim().split_once('x')?;
    let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let ratio = w / h;
    // The nearest of the ratios OpenRouter documents; square sends nothing.
    let options: [(&str, f64); 5] = [
        ("1:1", 1.0),
        ("16:9", 16.0 / 9.0),
        ("9:16", 9.0 / 16.0),
        ("4:3", 4.0 / 3.0),
        ("3:4", 3.0 / 4.0),
    ];
    let nearest = options.iter().min_by(|a, b| {
        (a.1 - ratio)
            .abs()
            .partial_cmp(&(b.1 - ratio).abs())
            .unwrap()
    })?;
    (nearest.0 != "1:1").then_some(nearest.0)
}

fn extract_image_output(body: &str) -> Result<ImageOutput, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|err| format!("OpenRouter image response was not JSON: {err}"))?;
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        return Err(format!("OpenRouter image generation failed: {message}"));
    }
    let first = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or("OpenRouter image response had no data[0]")?;
    let base64_data = first
        .get("b64_json")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("OpenRouter image response had no data[0].b64_json")?
        .to_string();
    Ok(ImageOutput {
        base64_data,
        media_type: first
            .get("media_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        usage_json: value.get("usage").map(Value::to_string).unwrap_or_default(),
    })
}

fn decode_image_base64(base64_data: &str) -> Result<Vec<u8>, String> {
    let payload = base64_data
        .split_once("base64,")
        .map(|(_, rest)| rest)
        .unwrap_or(base64_data);
    let compact: String = payload
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .map_err(|err| format!("OpenRouter returned invalid image base64: {err}"))
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

fn mime_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn resolve_output_path(fields: &Value, ctx: &Context, ext: &str) -> String {
    let configured = field_or_default(fields, &["output_path", "OutputPath"], "");
    if !configured.is_empty() {
        return ensure_path_extension(configured, ext);
    }
    format!("/generated/images/{}.{ext}", entity_id(ctx))
}

fn ensure_path_extension(path: &str, ext: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if [".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
    {
        path.to_string()
    } else {
        format!("{path}.{ext}")
    }
}

fn entity_id(ctx: &Context) -> String {
    ctx.entity_state
        .get("entity_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(ctx.entity_id.as_str())
        .to_string()
}

/// The image type a request asks for (output_format), PNG unless it says
/// otherwise: the File is created with it before the picture exists.
fn requested_mime(fields: &Value) -> String {
    match field_or_default(fields, &["output_format", "OutputFormat"], "png")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpeg" | "jpg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        _ => "image/png".to_string(),
    }
}

/// Create the picture's PawFS File (no content yet) and return its id. A File
/// a failed earlier attempt of this same request left at its path with no
/// content is reused, so a retry is not refused by PawFS's
/// one-file-per-(workspace, path) rule. Only a path that carries this
/// request's id is this request's own (the default /generated/images/<id>);
/// any other File already at the path is someone else's and is refused.
fn create_image_file(
    ctx: &Context,
    fields: &Value,
    workspace_id: &str,
    path: &str,
    mime_type: &str,
) -> Result<String, String> {
    let temper_api_url = resolve_temper_api_url(ctx, fields);
    if let Some(existing) = file_at(ctx, fields, &temper_api_url, workspace_id, path)? {
        let status = existing
            .get("status")
            .or_else(|| existing.get("Status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let id = entity_field_str(&existing, &["Id", "id"])
            .or_else(|| existing.get("entity_id").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if status == "Created" && !id.is_empty() && path_is_this_requests(path, &entity_id(ctx)) {
            return Ok(id);
        }
        return Err(format!(
            "image_generate: a picture already exists at {path} (File {id}, {status}); give another output_path"
        ));
    }
    let file_name = path
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("generated-image.png");
    let file_body = json!({ "Name": file_name, "Path": path, "WorkspaceId": workspace_id, "MimeType": mime_type });
    let headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        Some("application/json"),
        Some("application/json"),
    );
    let create_resp = ctx.http_call(
        "POST",
        &format!("{temper_api_url}/tdata/Files"),
        &headers,
        &file_body.to_string(),
    )?;
    if !(200..300).contains(&create_resp.status) {
        return Err(format!(
            "image_generate: PawFS File create failed before the paid call (HTTP {}): {}",
            create_resp.status,
            sanitized_body_snippet(&create_resp.body)
        ));
    }
    let file_value: Value = serde_json::from_str(&create_resp.body)
        .map_err(|err| format!("image_generate: parse PawFS File create response: {err}"))?;
    Ok(entity_field_str(&file_value, &["Id", "id"])
        .or_else(|| file_value.get("entity_id").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .ok_or("image_generate: PawFS File create response did not include an id")?
        .to_string())
}

/// Whether a path is this request's own: it carries the request's entity id.
fn path_is_this_requests(path: &str, request_id: &str) -> bool {
    !request_id.is_empty() && path.contains(request_id)
}

/// The File at (workspace, path), if there is one.
fn file_at(
    ctx: &Context,
    fields: &Value,
    temper_api_url: &str,
    workspace_id: &str,
    path: &str,
) -> Result<Option<Value>, String> {
    let filter = format!(
        "Path eq '{}' and WorkspaceId eq '{}'",
        path.replace('\'', "''"),
        workspace_id.replace('\'', "''")
    );
    let headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        None,
        Some("application/json"),
    );
    let resp = ctx.http_call(
        "GET",
        &format!(
            "{temper_api_url}/tdata/Files?$filter={}&$top=1",
            odata_query_encode(&filter)
        ),
        &headers,
        "",
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "image_generate: PawFS File lookup failed (HTTP {}): {}",
            resp.status,
            sanitized_body_snippet(&resp.body)
        ));
    }
    let body: Value = serde_json::from_str(&resp.body)
        .map_err(|err| format!("image_generate: parse PawFS File lookup: {err}"))?;
    Ok(body
        .get("value")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .cloned())
}

/// Percent-encode an OData query value (spaces, quotes and the rest; "/" stays).
fn odata_query_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Upload the picture's bytes to its File and return the new version's id.
fn upload_image_bytes(
    ctx: &Context,
    fields: &Value,
    workspace_id: &str,
    file_id: &str,
    mime_type: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let temper_api_url = resolve_temper_api_url(ctx, fields);
    let value_headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        Some(mime_type),
        None,
    );
    put_file_value_stream(
        &format!("{temper_api_url}/tdata/Files('{file_id}')/$value"),
        &value_headers,
        bytes,
    )?;

    let head_headers = runtime_headers_for_workspace(
        ctx,
        &ctx.tenant,
        fields,
        workspace_id,
        None,
        Some("application/json"),
    );
    let head_resp = ctx.http_call(
        "GET",
        &format!("{temper_api_url}/tdata/Files('{file_id}')"),
        &head_headers,
        "",
    )?;
    if !(200..300).contains(&head_resp.status) {
        return Err(format!(
            "image_generate: PawFS File read-after-write failed (HTTP {}): {}",
            head_resp.status,
            sanitized_body_snippet(&head_resp.body)
        ));
    }
    let head_value: Value = serde_json::from_str(&head_resp.body)
        .map_err(|err| format!("image_generate: parse PawFS File head response: {err}"))?;
    Ok(
        entity_field_str(&head_value, &["LastVersionId", "last_version_id"])
            .unwrap_or("")
            .to_string(),
    )
}

fn record_storing(
    ctx: &Context,
    fields: &Value,
    model: &str,
    output: &ImageOutput,
) -> Result<(), String> {
    let temper_api_url = resolve_temper_api_url(ctx, fields);
    let url = format!(
        "{temper_api_url}/tdata/MediaGenerationRequests('{}')/Temper.RecordStoring",
        entity_id(ctx).replace('\'', "''")
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
        "provider_response_id": format!("openrouter:{model}"),
        "revised_prompt": "",
        "usage_json": output.usage_json,
    });
    let resp = ctx.http_call("POST", &url, &headers, &body.to_string())?;
    if !(200..300).contains(&resp.status) {
        ctx.log(
            "warn",
            &format!(
                "openrouter_image_generate: RecordStoring failed (HTTP {}): {}",
                resp.status,
                sanitized_body_snippet(&resp.body)
            ),
        );
    }
    Ok(())
}

/// The response carries the image as base64 inside JSON, often megabytes:
/// read it through the streaming host API, as the Codex renderer does, so it
/// is not cut off by the fixed non-streaming response buffer.
#[cfg(target_arch = "wasm32")]
fn call_openrouter(
    _ctx: &Context,
    url: &str,
    headers: &[(String, String)],
    request: &Value,
) -> Result<HttpTextResponse, String> {
    let header_refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let (mut request_body, mut response_body, response_head) =
        temper_wasm_sdk::http_stream::streaming_call("POST", url, &header_refs)
            .map_err(|error| format!("OpenRouter request failed to start: {error}"))?;
    let body = request.to_string();
    for chunk in body.as_bytes().chunks(RESPONSE_STREAM_CHUNK_BYTES) {
        request_body
            .write_all_chunk(chunk)
            .map_err(|error| format!("OpenRouter request write failed: {error}"))?;
    }
    request_body
        .finish()
        .map_err(|error| format!("OpenRouter request close failed: {error}"))?;
    let head =
        response_head().map_err(|error| format!("OpenRouter response head failed: {error}"))?;
    let mut body_bytes = Vec::new();
    let mut buffer = vec![0u8; RESPONSE_STREAM_CHUNK_BYTES];
    while let Some(read) = response_body
        .read_next_chunk(&mut buffer)
        .map_err(|error| format!("OpenRouter response read failed: {error}"))?
    {
        body_bytes.extend_from_slice(&buffer[..read]);
        if body_bytes.len() > RESPONSE_MAX_BYTES {
            let _ = response_body.close();
            return Err(format!(
                "OpenRouter image response exceeded {RESPONSE_MAX_BYTES} bytes"
            ));
        }
    }
    response_body
        .close()
        .map_err(|error| format!("OpenRouter response close failed: {error}"))?;
    let body = String::from_utf8(body_bytes)
        .map_err(|error| format!("OpenRouter response was not UTF-8: {error}"))?;
    Ok(HttpTextResponse {
        status: head.status,
        body,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn call_openrouter(
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
        .map(|(k, v)| (k.as_str(), v.as_str()))
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

fn config_value(ctx: &Context, key: &str) -> Option<String> {
    ctx.config
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && !value.contains("{secret:"))
        .map(ToOwned::to_owned)
}

fn field_or_default<'a>(value: &'a Value, keys: &[&str], default: &'a str) -> &'a str {
    entity_field_str(value, keys)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn sanitized_body_snippet(body: &str) -> String {
    body.chars()
        .take(500)
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_1X1_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=";

    #[test]
    fn request_sends_only_what_every_model_takes() {
        let square = build_openrouter_image_request(
            &json!({ "size": "1024x1024", "quality": "high", "background": "opaque" }),
            "a teapot",
            DEFAULT_MODEL,
        );
        assert_eq!(
            square,
            json!({ "model": DEFAULT_MODEL, "prompt": "a teapot", "n": 1 })
        );
        let wide = build_openrouter_image_request(
            &json!({ "size": "1536x1024" }),
            "a harbour",
            "google/gemini-3.1-flash-image",
        );
        assert_eq!(wide["aspect_ratio"], "4:3");
        assert_eq!(wide["model"], "google/gemini-3.1-flash-image");
        assert!(wide.get("quality").is_none() && wide.get("size").is_none());
    }

    #[test]
    fn aspect_ratio_is_the_nearest_documented_one() {
        assert_eq!(aspect_ratio_for_size("1024x1024"), None);
        assert_eq!(aspect_ratio_for_size("1920x1080"), Some("16:9"));
        assert_eq!(aspect_ratio_for_size("1024x1536"), Some("3:4"));
        assert_eq!(aspect_ratio_for_size("auto"), None);
        assert_eq!(aspect_ratio_for_size(""), None);
    }

    #[test]
    fn response_image_is_read_from_data_b64_json() {
        let body = json!({ "created": 1, "data": [{ "b64_json": PNG_1X1_BASE64, "media_type": "image/png" }], "usage": { "cost": 0.04 } }).to_string();
        let output = extract_image_output(&body).expect("image output");
        let bytes = decode_image_base64(&output.base64_data).expect("decoded");
        assert_eq!(detect_image_mime(&bytes).as_deref(), Some("image/png"));
        assert_eq!(output.usage_json, json!({ "cost": 0.04 }).to_string());
    }

    #[test]
    fn a_data_url_decodes_too() {
        let bytes = decode_image_base64(&format!("data:image/png;base64,{PNG_1X1_BASE64}"))
            .expect("decoded");
        assert_eq!(detect_image_mime(&bytes).as_deref(), Some("image/png"));
    }

    #[test]
    fn provider_errors_are_reported_not_stored() {
        let err = extract_image_output(
            &json!({ "error": { "message": "model not found", "code": 404 } }).to_string(),
        )
        .unwrap_err();
        assert!(err.contains("model not found"));
        assert!(
            extract_image_output(&json!({ "data": [] }).to_string())
                .unwrap_err()
                .contains("data[0]")
        );
    }

    #[test]
    fn only_a_path_carrying_this_request_id_is_its_own() {
        assert!(path_is_this_requests(
            "/generated/images/req-1.png",
            "req-1"
        ));
        assert!(!path_is_this_requests(
            "/transfer-test/art-styles/a/t/x-1.png",
            "req-1"
        ));
        assert!(!path_is_this_requests("/generated/images/req-1.png", ""));
    }

    #[test]
    fn a_file_lookup_query_is_encoded() {
        assert_eq!(
            odata_query_encode("Path eq '/transfer-test/a b.png' and WorkspaceId eq 'ws'"),
            "Path%20eq%20%27/transfer-test/a%20b.png%27%20and%20WorkspaceId%20eq%20%27ws%27"
        );
    }

    #[test]
    fn the_file_is_created_with_the_requested_image_type() {
        assert_eq!(requested_mime(&json!({})), "image/png");
        assert_eq!(
            requested_mime(&json!({ "output_format": "jpeg" })),
            "image/jpeg"
        );
        assert_eq!(
            requested_mime(&json!({ "output_format": "webp" })),
            "image/webp"
        );
    }

    #[test]
    fn only_allowed_models_are_used() {
        assert_eq!(choose_model("", DEFAULT_MODEL).unwrap(), DEFAULT_MODEL);
        assert_eq!(
            choose_model("gpt-image-2", DEFAULT_MODEL).unwrap(),
            DEFAULT_MODEL
        );
        assert_eq!(
            choose_model(DEFAULT_MODEL, "vendor/configured").unwrap(),
            DEFAULT_MODEL
        );
        assert_eq!(
            choose_model("vendor/configured", "vendor/configured").unwrap(),
            "vendor/configured"
        );
        assert!(choose_model("openai/some-expensive-model", DEFAULT_MODEL).is_err());
    }

    #[test]
    fn the_four_transfer_test_models_are_drawn_and_no_others() {
        for model in [
            "x-ai/grok-imagine-image-2.0",
            "openai/gpt-image-2.5-sunburst",
            "google/gemini-3.1-flash-image",
            "bytedance-seed/seedream-5-0-pro",
        ] {
            assert_eq!(choose_model(model, DEFAULT_MODEL).as_deref(), Ok(model));
        }
        // Another tier of the same families is still refused before a paid call.
        for model in [
            "openai/gpt-image-2.5-flare",
            "google/gemini-3-pro-image",
            "bytedance-seed/seedream-5-0-flash",
        ] {
            assert!(choose_model(model, DEFAULT_MODEL).is_err(), "{model}");
        }
    }

    #[test]
    fn only_image_generation_is_accepted() {
        assert!(
            validate_request(
                &json!({ "media_type": "image", "operation": "generate", "provider": "openrouter" })
            )
            .is_ok()
        );
        assert!(validate_request(&json!({ "media_type": "video" })).is_err());
        assert!(validate_request(&json!({ "operation": "edit" })).is_err());
    }
}
