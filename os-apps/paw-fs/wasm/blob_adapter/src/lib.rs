//! TemperFS blob_adapter — WASM guest module for blob storage operations.
//!
//! Handles auth, hashing, caching, and upload/download orchestration for
//! `$value` endpoints. Bytes never enter WASM memory — they flow through
//! the host's StreamRegistry, referenced by stream IDs.
//!
//! All logic here is **hot-reloadable** by deploying a new `.wasm` binary.
//!
//! Host functions used:
//! - `host_hash_stream`: compute content hash (algorithm chosen here, computed by host)
//! - `host_cache_contains`: check if bytes are cached
//! - `host_cache_to_stream`: copy cached bytes to a stream for response
//! - `host_cache_from_stream`: cache bytes from a stream
//! - `host_http_call_stream`: HTTP with stream-based body/response
//! - `host_get_secret`: read secrets (blob_access_key, blob_secret_key)
//! - `host_get_time`: get current UTC time for Sig V4 signing
//! - `host_get_context`: read invocation context
//! - `host_set_result`: return result to host
//! - `host_log`: structured logging
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use core::ptr::addr_of;
use hmac::{Hmac, Mac};
use sha2::{Sha256, Digest};

type HmacSha256 = Hmac<Sha256>;

// ---- Host function imports ----

unsafe extern "C" {
    fn host_log(level_ptr: i32, level_len: i32, msg_ptr: i32, msg_len: i32);
    fn host_log_structured(ptr: i32, len: i32) -> i32;
    fn host_get_context(buf_ptr: i32, buf_len: i32) -> i32;
    fn host_set_result(ptr: i32, len: i32);
    fn host_get_secret(key_ptr: i32, key_len: i32, buf_ptr: i32, buf_len: i32) -> i32;

    /// Get current UTC time as "YYYYMMDDTHHMMSSz". Returns bytes written, -1 on error.
    fn host_get_time(buf_ptr: i32, buf_len: i32) -> i32;

    /// HTTP with stream-based body/response. Returns HTTP status code, -1 on error.
    fn host_http_call_stream(
        method_ptr: i32,
        method_len: i32,
        url_ptr: i32,
        url_len: i32,
        headers_ptr: i32,
        headers_len: i32,
        body_stream_id_ptr: i32,
        body_stream_id_len: i32,
        response_stream_id_ptr: i32,
        response_stream_id_len: i32,
    ) -> i32;

    /// Check if bytes are cached. Returns 1 if cached, 0 if not.
    fn host_cache_contains(key_ptr: i32, key_len: i32) -> i32;

    /// Copy cached bytes to a stream. Returns byte count, -1 if not cached.
    fn host_cache_to_stream(
        key_ptr: i32,
        key_len: i32,
        stream_id_ptr: i32,
        stream_id_len: i32,
    ) -> i32;

    /// Cache bytes from a stream. Returns 0 on success, -1 on error.
    fn host_cache_from_stream(
        key_ptr: i32,
        key_len: i32,
        stream_id_ptr: i32,
        stream_id_len: i32,
    ) -> i32;

    /// Compute hash of stream bytes. Returns bytes written, -1 on error.
    fn host_hash_stream(
        stream_id_ptr: i32,
        stream_id_len: i32,
        algorithm_ptr: i32,
        algorithm_len: i32,
        result_buf_ptr: i32,
        result_buf_len: i32,
    ) -> i32;
}

// ---- Buffers ----

const CTX_BUF_LEN: usize = 131072; // 128KB — large enough for session-backed entity state
const SECRET_BUF_LEN: usize = 1024;
const HASH_BUF_LEN: usize = 256;
const TIME_BUF_LEN: usize = 32;
const BLOB_OBSERVABILITY_EVENT: &str = "temperpaw.blob";
const BLOB_BACKEND: &str = "temperfs-blob";

static mut CTX_BUF: [u8; CTX_BUF_LEN] = [0u8; CTX_BUF_LEN];
static mut SECRET_BUF: [u8; SECRET_BUF_LEN] = [0u8; SECRET_BUF_LEN];
static mut HASH_BUF: [u8; HASH_BUF_LEN] = [0u8; HASH_BUF_LEN];
static mut TIME_BUF: [u8; TIME_BUF_LEN] = [0u8; TIME_BUF_LEN];

// ---- Entry point ----

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let ctx_json = match read_context() {
        Some(s) => s,
        None => {
            set_error_result("failed to read invocation context");
            return 1;
        }
    };

    // Parse operation from trigger_params
    let operation = extract_json_str(&ctx_json, "operation");
    match operation.as_str() {
        "put" => handle_upload(&ctx_json),
        "get" => handle_download(&ctx_json),
        _ => {
            set_error_result(&format!("unknown operation: {operation}"));
            1
        }
    }
}

// ---- Upload ----

fn handle_upload(ctx_json: &str) -> i32 {
    let stream_id = extract_json_str(ctx_json, "stream_id");
    let size_bytes = extract_json_str(ctx_json, "size_bytes");
    let content_type = extract_json_str(ctx_json, "content_type");

    // 1. Compute content hash — algorithm is hot-reloadable!
    let content_hash = match compute_hash(&stream_id, "sha256") {
        Some(h) => h,
        None => {
            emit_blob_observability(
                ctx_json,
                "put",
                "hash_failed",
                "",
                false,
                &stream_id,
                None,
                &size_bytes,
                &content_type,
            );
            set_error_result("failed to compute content hash");
            return 1;
        }
    };

    // 2. CAS dedup — skip upload if blob already stored
    if cache_contains(&content_hash) {
        emit_blob_observability(
            ctx_json,
            "put",
            "cache_hit",
            &content_hash,
            true,
            &stream_id,
            Some(200),
            &size_bytes,
            &content_type,
        );
        let result = build_stream_updated_result(ctx_json, &content_hash, &size_bytes, &content_type);
        set_result(&result);
        return 0;
    }

    // 3. Read blob storage credentials
    let endpoint = read_secret_or("blob_endpoint", "https://blob.example.com");
    let bucket = read_secret_or("blob_bucket", "temper-fs");

    // 4. Construct URL (content-addressable: key = hash)
    let url = format!("{endpoint}/{bucket}/{content_hash}");

    // 5. Upload — bytes flow from StreamRegistry via host, never through WASM memory
    let headers_json = sign_request("PUT", &url, "UNSIGNED-PAYLOAD");
    let status = call_http_stream("PUT", &url, &headers_json, &stream_id, "");

    if status < 200 || status >= 300 {
        emit_blob_observability(
            ctx_json,
            "put",
            "upload_failed",
            &content_hash,
            false,
            &stream_id,
            Some(status),
            &size_bytes,
            &content_type,
        );
        set_error_result(&format!("upload failed with HTTP {status}"));
        return 1;
    }

    // 6. Cache for future reads and dedup
    cache_from_stream(&content_hash, &stream_id);

    // 7. Return action + params for server to dispatch
    emit_blob_observability(
        ctx_json,
        "put",
        "uploaded",
        &content_hash,
        false,
        &stream_id,
        Some(status),
        &size_bytes,
        &content_type,
    );
    let result = build_stream_updated_result(ctx_json, &content_hash, &size_bytes, &content_type);
    set_result(&result);
    0
}

// ---- Download ----

fn handle_download(ctx_json: &str) -> i32 {
    let response_stream_id = extract_json_str(ctx_json, "stream_id");
    let size_bytes = extract_stream_size_bytes(ctx_json);
    let content_type = extract_stream_content_type(ctx_json);

    // Read content_hash from entity_state.fields.content_hash
    // entity_state is nested: {"fields":{"content_hash":"sha256:..."},...}
    let content_hash = {
        let es = extract_json_object(ctx_json, "entity_state");
        if es.is_empty() {
            String::new()
        } else {
            let fields = extract_json_object(&es, "fields");
            if fields.is_empty() {
                // Fallback: try direct extraction from entity_state
                extract_json_str(&es, "content_hash")
            } else {
                extract_json_str(&fields, "content_hash")
            }
        }
    };
    if content_hash.is_empty() {
        emit_blob_observability(
            ctx_json,
            "get",
            "missing_content_hash",
            "",
            false,
            &response_stream_id,
            None,
            &size_bytes,
            &content_type,
        );
        set_error_result("entity has no content_hash");
        return 1;
    }

    // 1. Cache check — skip download if bytes already cached
    if cache_contains(&content_hash) {
        let copied = cache_to_stream(&content_hash, &response_stream_id);
        if copied >= 0 {
            emit_blob_observability(
                ctx_json,
                "get",
                "cache_hit",
                &content_hash,
                true,
                &response_stream_id,
                Some(200),
                &size_bytes,
                &content_type,
            );
            let result = r#"{"success":true}"#;
            set_result(result);
            return 0;
        }
        // Fall through to R2 download if cache_to_stream failed
    }

    // 2. Read blob storage credentials
    let endpoint = read_secret_or("blob_endpoint", "https://blob.example.com");
    let bucket = read_secret_or("blob_bucket", "temper-fs");

    // 3. Construct URL
    let url = format!("{endpoint}/{bucket}/{content_hash}");

    // 4. Download — bytes go to StreamRegistry via host
    let headers_json = sign_request("GET", &url, "UNSIGNED-PAYLOAD");
    let status = call_http_stream("GET", &url, &headers_json, "", &response_stream_id);

    if status < 200 || status >= 300 {
        emit_blob_observability(
            ctx_json,
            "get",
            "download_failed",
            &content_hash,
            false,
            &response_stream_id,
            Some(status),
            &size_bytes,
            &content_type,
        );
        set_error_result(&format!("download failed with HTTP {status}"));
        return 1;
    }

    // 5. Cache for next time
    cache_from_stream(&content_hash, &response_stream_id);

    emit_blob_observability(
        ctx_json,
        "get",
        "downloaded",
        &content_hash,
        false,
        &response_stream_id,
        Some(status),
        &size_bytes,
        &content_type,
    );
    let result = r#"{"success":true}"#;
    set_result(result);
    0
}

// ---- Host function wrappers ----

fn log(level: &str, msg: &str) {
    unsafe {
        host_log(
            level.as_ptr() as i32,
            level.len() as i32,
            msg.as_ptr() as i32,
            msg.len() as i32,
        );
    }
}

fn log_structured(level: &str, message: &str, fields_json: &str) {
    let payload = format!(
        r#"{{"level":"{}","message":"{}","fields":{}}}"#,
        escape_json(level),
        escape_json(message),
        fields_json,
    );
    let rc = unsafe { host_log_structured(payload.as_ptr() as i32, payload.len() as i32) };
    if rc != 0 {
        log(level, message);
    }
}

fn emit_blob_observability(
    ctx_json: &str,
    operation: &str,
    outcome: &str,
    content_hash: &str,
    cache_hit: bool,
    stream_id: &str,
    status_code: Option<i32>,
    size_bytes: &str,
    content_type: &str,
) {
    let fields = build_blob_observability_fields(
        ctx_json,
        operation,
        outcome,
        content_hash,
        cache_hit,
        stream_id,
        status_code,
        size_bytes,
        content_type,
    );
    log_structured("info", "temperpaw.blob operation", &fields);
}

fn read_context() -> Option<String> {
    unsafe {
        let ptr = addr_of!(CTX_BUF) as *const u8;
        let len = host_get_context(ptr as i32, CTX_BUF_LEN as i32);
        if len <= 0 {
            return None;
        }
        if len as usize <= CTX_BUF_LEN {
            let slice = core::slice::from_raw_parts(ptr, len as usize);
            return Some(String::from_utf8_lossy(slice).to_string());
        }

        let needed = len as usize;
        let mut dynamic = vec![0u8; needed];
        let actual = host_get_context(dynamic.as_mut_ptr() as i32, needed as i32);
        if actual <= 0 || actual as usize != needed {
            return None;
        }
        Some(String::from_utf8_lossy(&dynamic).to_string())
    }
}

fn set_result(json: &str) {
    unsafe {
        host_set_result(json.as_ptr() as i32, json.len() as i32);
    }
}

fn set_error_result(error: &str) {
    let result = format!(
        r#"{{"success":false,"error":"{}"}}"#,
        escape_json(error),
    );
    set_result(&result);
}

fn compute_hash(stream_id: &str, algorithm: &str) -> Option<String> {
    unsafe {
        let ptr = addr_of!(HASH_BUF) as *const u8;
        let len = host_hash_stream(
            stream_id.as_ptr() as i32,
            stream_id.len() as i32,
            algorithm.as_ptr() as i32,
            algorithm.len() as i32,
            ptr as i32,
            HASH_BUF_LEN as i32,
        );
        if len <= 0 {
            return None;
        }
        let slice = core::slice::from_raw_parts(ptr, len as usize);
        Some(String::from_utf8_lossy(slice).to_string())
    }
}

fn cache_contains(key: &str) -> bool {
    unsafe { host_cache_contains(key.as_ptr() as i32, key.len() as i32) == 1 }
}

fn cache_to_stream(key: &str, stream_id: &str) -> i32 {
    unsafe {
        host_cache_to_stream(
            key.as_ptr() as i32,
            key.len() as i32,
            stream_id.as_ptr() as i32,
            stream_id.len() as i32,
        )
    }
}

fn cache_from_stream(key: &str, stream_id: &str) {
    unsafe {
        host_cache_from_stream(
            key.as_ptr() as i32,
            key.len() as i32,
            stream_id.as_ptr() as i32,
            stream_id.len() as i32,
        );
    }
}

fn call_http_stream(
    method: &str,
    url: &str,
    headers_json: &str,
    body_stream_id: &str,
    response_stream_id: &str,
) -> i32 {
    unsafe {
        host_http_call_stream(
            method.as_ptr() as i32,
            method.len() as i32,
            url.as_ptr() as i32,
            url.len() as i32,
            headers_json.as_ptr() as i32,
            headers_json.len() as i32,
            body_stream_id.as_ptr() as i32,
            body_stream_id.len() as i32,
            response_stream_id.as_ptr() as i32,
            response_stream_id.len() as i32,
        )
    }
}

fn read_secret_or(key: &str, default: &str) -> String {
    unsafe {
        let ptr = addr_of!(SECRET_BUF) as *const u8;
        let len = host_get_secret(
            key.as_ptr() as i32,
            key.len() as i32,
            ptr as i32,
            SECRET_BUF_LEN as i32,
        );
        if len <= 0 {
            return default.to_string();
        }
        let slice = core::slice::from_raw_parts(ptr, len as usize);
        String::from_utf8_lossy(slice).to_string()
    }
}

fn get_utc_now() -> String {
    unsafe {
        let ptr = addr_of!(TIME_BUF) as *const u8;
        let len = host_get_time(ptr as i32, TIME_BUF_LEN as i32);
        if len <= 0 {
            return String::new();
        }
        let slice = core::slice::from_raw_parts(ptr, len as usize);
        String::from_utf8_lossy(slice).to_string()
    }
}

// ---- AWS Signature V4 (for GCS S3-compatible XML API) ----

/// Sign an HTTP request using AWS Signature V4.
/// Returns headers as `[["name","value"],...]` JSON for `host_http_call_stream`.
/// If no access key is configured, returns `"[]"` (unsigned — for local dev).
fn sign_request(method: &str, url: &str, payload_hash: &str) -> String {
    let access_key = read_secret_or("blob_access_key", "");
    if access_key.is_empty() {
        return "[]".to_string();
    }
    let secret_key = read_secret_or("blob_secret_key", "");
    if secret_key.is_empty() {
        log("warn", "blob_adapter: blob_access_key set but blob_secret_key missing, sending unsigned");
        return "[]".to_string();
    }
    let datetime = get_utc_now();
    if datetime.is_empty() {
        log("warn", "blob_adapter: host_get_time failed, sending unsigned");
        return "[]".to_string();
    }
    // date = first 8 chars of datetime (YYYYMMDD)
    let date = &datetime[..8];

    let (host, path) = parse_url_host_path(url);
    let canonical_uri = uri_encode_path(path);

    let region = "auto";
    let service = "s3";
    let scope = format!("{date}/{region}/{service}/aws4_request");

    // Canonical headers (sorted by name)
    let canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{datetime}\n"
    );
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";

    // Canonical request
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    log("debug", &format!("blob_adapter: canonical_request=\n{canonical_request}"));

    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());

    // String to sign
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{datetime}\n{scope}\n{canonical_request_hash}"
    );

    // Derive signing key and compute signature
    let signing_key = derive_signing_key(&secret_key, date, region, service);
    let signature = hex_encode(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope},SignedHeaders={signed_headers},Signature={signature}"
    );

    format!(
        r#"[["Authorization","{}"],["x-amz-date","{}"],["x-amz-content-sha256","{}"],["Host","{}"]]"#,
        escape_json(&authorization),
        escape_json(&datetime),
        escape_json(payload_hash),
        escape_json(host),
    )
}

/// Derive the Sig V4 signing key via 4-step HMAC chain.
fn derive_signing_key(secret_key: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{secret_key}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

// ---- Crypto helpers ----

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex_encode(&hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Split a URL into (host, path). Path includes the leading `/`.
fn parse_url_host_path(url: &str) -> (&str, &str) {
    let after_scheme = if let Some(pos) = url.find("://") {
        &url[pos + 3..]
    } else {
        url
    };
    if let Some(slash) = after_scheme.find('/') {
        (&after_scheme[..slash], &after_scheme[slash..])
    } else {
        (after_scheme, "/")
    }
}

/// Percent-encode path for S3 canonical URI.
/// Encodes `:` as `%3A` (needed for `sha256:abc123` content hash keys).
/// Leaves `/`, alphanumerics, `-`, `_`, `.`, `~` unencoded per RFC 3986.
fn uri_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 16);
    for b in path.bytes() {
        match b {
            b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(b as char),
            _ => {
                out.push('%');
                out.push(b"0123456789ABCDEF"[(b >> 4) as usize] as char);
                out.push(b"0123456789ABCDEF"[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
}

// ---- Minimal JSON helpers (no serde in WASM guest) ----

/// Extract a string value from JSON (top-level key in trigger_params or context).
fn extract_json_str(json: &str, key: &str) -> String {
    // Look in trigger_params first, then top level
    let search_key = format!(r#""{key}":""#);
    if let Some(start_idx) = json.find(&search_key) {
        let value_start = start_idx + search_key.len();
        if let Some(end_idx) = json[value_start..].find('"') {
            return json[value_start..value_start + end_idx].to_string();
        }
    }
    // Try numeric value
    let search_key_num = format!(r#""{key}":"#);
    if let Some(start_idx) = json.find(&search_key_num) {
        let value_start = start_idx + search_key_num.len();
        let rest = &json[value_start..];
        let end = rest
            .find(|c: char| c == ',' || c == '}' || c == ' ')
            .unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    String::new()
}

/// Extract a JSON object value as a string (brace-matched).
fn extract_json_object(json: &str, key: &str) -> String {
    let search = format!(r#""{key}":"#);
    if let Some(start) = json.find(&search) {
        let rest = &json[start + search.len()..];
        // Skip whitespace
        let rest = rest.trim_start();
        if rest.starts_with('{') {
            // Brace-match to find the end
            let mut depth = 0;
            let mut in_string = false;
            let mut escape_next = false;
            for (i, c) in rest.char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                match c {
                    '\\' if in_string => escape_next = true,
                    '"' => in_string = !in_string,
                    '{' if !in_string => depth += 1,
                    '}' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            return rest[..=i].to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    String::new()
}

fn build_stream_updated_result(
    ctx_json: &str,
    content_hash: &str,
    size_bytes: &str,
    mime_type: &str,
) -> String {
    let (version_number, previous_version_id, created_by) =
        extract_stream_version_metadata(ctx_json);
    format!(
        r#"{{"action":"StreamUpdated","params":{{"content_hash":"{}","size_bytes":{},"mime_type":"{}","version_number":{},"previous_version_id":"{}","created_by":"{}"}},"success":true}}"#,
        escape_json(content_hash),
        size_bytes,
        escape_json(mime_type),
        version_number,
        escape_json(&previous_version_id),
        escape_json(&created_by),
    )
}

fn extract_stream_version_metadata(ctx_json: &str) -> (u64, String, String) {
    let entity_state = extract_json_object(ctx_json, "entity_state");
    let fields = if entity_state.is_empty() {
        String::new()
    } else {
        extract_json_object(&entity_state, "fields")
    };
    let version_count = extract_field_from_entity_state(&entity_state, &fields, "version_count")
        .parse::<u64>()
        .unwrap_or(0);
    let previous_version_id =
        extract_field_from_entity_state(&entity_state, &fields, "last_version_id");
    let created_by = extract_json_str(ctx_json, "agent_id");
    (
        version_count.saturating_add(1),
        previous_version_id,
        created_by,
    )
}

fn extract_field_from_entity_state(entity_state: &str, fields: &str, field: &str) -> String {
    if !fields.is_empty() {
        let value = extract_json_str(fields, field);
        if !value.is_empty() {
            return value;
        }
    }
    if !entity_state.is_empty() {
        return extract_json_str(entity_state, field);
    }
    String::new()
}

fn extract_stream_size_bytes(ctx_json: &str) -> String {
    let entity_state = extract_json_object(ctx_json, "entity_state");
    let fields = if entity_state.is_empty() {
        String::new()
    } else {
        extract_json_object(&entity_state, "fields")
    };
    extract_first_field(
        ctx_json,
        &entity_state,
        &fields,
        &["size_bytes", "SizeBytes"],
    )
}

fn extract_stream_content_type(ctx_json: &str) -> String {
    let entity_state = extract_json_object(ctx_json, "entity_state");
    let fields = if entity_state.is_empty() {
        String::new()
    } else {
        extract_json_object(&entity_state, "fields")
    };
    extract_first_field(
        ctx_json,
        &entity_state,
        &fields,
        &["content_type", "mime_type", "ContentType", "MimeType"],
    )
}

fn extract_first_field(
    ctx_json: &str,
    entity_state: &str,
    fields: &str,
    candidates: &[&str],
) -> String {
    for field in candidates {
        let value = extract_field_from_entity_state(entity_state, fields, field);
        if !value.is_empty() {
            return value;
        }
    }
    for field in candidates {
        let value = extract_json_str(ctx_json, field);
        if !value.is_empty() {
            return value;
        }
    }
    String::new()
}

fn build_blob_observability_fields(
    ctx_json: &str,
    operation: &str,
    outcome: &str,
    content_hash: &str,
    cache_hit: bool,
    stream_id: &str,
    status_code: Option<i32>,
    size_bytes: &str,
    content_type: &str,
) -> String {
    let entity_state = extract_json_object(ctx_json, "entity_state");
    let fields = if entity_state.is_empty() {
        String::new()
    } else {
        extract_json_object(&entity_state, "fields")
    };
    let workspace_id =
        extract_first_field(ctx_json, &entity_state, &fields, &["workspace_id", "WorkspaceId"]);
    let file_id = extract_json_str(ctx_json, "entity_id");
    let status_code = status_code.unwrap_or(0);
    let size_bytes = numeric_json(size_bytes);

    format!(
        r#"{{"observability_event":"{}","workspace_id":"{}","file_id":"{}","content_hash":"{}","stream_id":"{}","content_type":"{}","blob":{{"operation":"{}","outcome":"{}","backend":"{}","cache_hit":{},"status_code":{},"size_bytes":{}}}}}"#,
        BLOB_OBSERVABILITY_EVENT,
        escape_json(&workspace_id),
        escape_json(&file_id),
        escape_json(content_hash),
        escape_json(stream_id),
        escape_json(content_type),
        escape_json(operation),
        escape_json(outcome),
        BLOB_BACKEND,
        if cache_hit { "true" } else { "false" },
        status_code,
        size_bytes,
    )
}

fn numeric_json(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "0".to_string();
    }
    if trimmed
        .chars()
        .enumerate()
        .all(|(idx, ch)| ch.is_ascii_digit() || (idx == 0 && ch == '-'))
    {
        trimmed.to_string()
    } else {
        "0".to_string()
    }
}

/// Minimal JSON string escaping.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}
