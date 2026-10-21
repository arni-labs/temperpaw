//! OpenAI Codex Auth — device-code OAuth for ChatGPT/Codex subscription access.

use openai_codex_wire::{
    codex_access_token_is_fresh, extract_chatgpt_account_id_from_jwt, extract_jwt_expires_at_ms,
};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::{resolve_temper_api_url, runtime_headers};

#[cfg(not(target_arch = "wasm32"))]
#[unsafe(no_mangle)]
extern "C" fn host_get_time_millis() -> i64 {
    0
}

const DEFAULT_OPENAI_AUTH_BASE_URL: &str = "https://auth.openai.com";
const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_CODEX_DEVICE_CALLBACK_URL: &str = "https://auth.openai.com/deviceauth/callback";
const DEVICE_CODE_TTL_MS: i64 = 15 * 60 * 1000;
const DEFAULT_POLL_INTERVAL_MS: i64 = 5_000;
const DEFAULT_REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

const SECRET_ACCESS_TOKEN: &str = "openai_codex_access_token";
const SECRET_REFRESH_TOKEN: &str = "openai_codex_refresh_token";
const SECRET_EXPIRES_AT_MS: &str = "openai_codex_expires_at_ms";
const SECRET_ACCOUNT_ID: &str = "openai_codex_account_id";

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_openai_codex_auth() {
        set_error_result(&err);
    }
    0
}

fn run_openai_codex_auth() -> Result<(), String> {
    let ctx = Context::from_host()?;
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let mode = ctx
        .config
        .get("mode")
        .map(String::as_str)
        .unwrap_or("start");
    ctx.log("info", &format!("openai_codex_auth: mode={mode}"));

    match mode {
        "start" => start_device_login(&ctx),
        "poll" => poll_device_login(&ctx, &fields),
        "refresh" => refresh_tokens(&ctx),
        "ensure" => ensure_tokens_fresh(&ctx, false),
        "force_refresh" => ensure_tokens_fresh(&ctx, true),
        "disconnect" => disconnect(&ctx),
        other => Err(format!("openai_codex_auth: unsupported mode {other}")),
    }
}

fn start_device_login(ctx: &Context) -> Result<(), String> {
    let auth_base_url = auth_base_url(ctx);
    let body = json!({ "client_id": OPENAI_CODEX_CLIENT_ID }).to_string();
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let resp = ctx.http_call(
        "POST",
        &format!("{auth_base_url}/api/accounts/deviceauth/usercode"),
        &headers,
        &body,
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI device code request failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }

    let parsed: Value =
        serde_json::from_str(&resp.body).map_err(|e| format!("device code JSON parse: {e}"))?;
    let device_auth_id = string_field(&parsed, "device_auth_id")
        .ok_or("OpenAI device code response missing device_auth_id")?;
    let user_code = string_field(&parsed, "user_code")
        .or_else(|| string_field(&parsed, "usercode"))
        .ok_or("OpenAI device code response missing user_code")?;
    let poll_interval_ms = parsed
        .get("interval")
        .and_then(Value::as_i64)
        .map(|seconds| (seconds * 1000).max(1000))
        .unwrap_or(DEFAULT_POLL_INTERVAL_MS);
    let expires_at_ms = Context::get_time_millis() as i64 + DEVICE_CODE_TTL_MS;

    set_success_result(
        "DeviceCodeReady",
        &with_cleared_error(json!({
            "verification_url": format!("{auth_base_url}/codex/device"),
            "user_code": user_code,
            "device_auth_id": device_auth_id,
            "poll_interval_ms": poll_interval_ms.to_string(),
            "expires_at_ms": expires_at_ms.to_string(),
        })),
    );
    Ok(())
}

fn poll_device_login(ctx: &Context, fields: &Value) -> Result<(), String> {
    let auth_base_url = auth_base_url(ctx);
    let device_auth_id = field_str(fields, "device_auth_id")
        .ok_or("OpenAI Codex auth missing device_auth_id; start device login first")?;
    let user_code = field_str(fields, "user_code")
        .ok_or("OpenAI Codex auth missing user_code; start device login first")?;
    let body = json!({
        "device_auth_id": device_auth_id,
        "user_code": user_code,
    })
    .to_string();
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let resp = ctx.http_call(
        "POST",
        &format!("{auth_base_url}/api/accounts/deviceauth/token"),
        &headers,
        &body,
    )?;

    if resp.status == 403 || resp.status == 404 {
        set_success_result(
            "DeviceCodeReady",
            &json!({
                "verification_url": field_str(fields, "verification_url").unwrap_or("https://auth.openai.com/codex/device"),
                "user_code": user_code,
                "device_auth_id": device_auth_id,
                "poll_interval_ms": field_str(fields, "poll_interval_ms").unwrap_or("5000"),
                "expires_at_ms": field_str(fields, "expires_at_ms").unwrap_or(""),
            }),
        );
        return Ok(());
    }

    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI device authorization failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }

    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|e| format!("device authorization JSON parse: {e}"))?;
    let authorization_code =
        string_field(&parsed, "authorization_code").ok_or("missing authorization_code")?;
    let code_verifier = string_field(&parsed, "code_verifier").ok_or("missing code_verifier")?;
    let tokens =
        exchange_authorization_code(ctx, &auth_base_url, &authorization_code, &code_verifier)?;
    store_tokens_and_complete(ctx, tokens)
}

fn ensure_tokens_fresh(ctx: &Context, force_refresh: bool) -> Result<(), String> {
    let access_token = resolve_config_or_secret(ctx, SECRET_ACCESS_TOKEN);
    let expires_at_ms = resolve_config_or_secret(ctx, SECRET_EXPIRES_AT_MS);
    let account_id = resolve_config_or_secret(ctx, SECRET_ACCOUNT_ID).or_else(|| {
        access_token
            .as_deref()
            .and_then(extract_chatgpt_account_id_from_jwt)
    });
    let now_ms = Context::get_time_millis();

    if !force_refresh
        && codex_access_token_is_fresh(
            access_token.as_deref(),
            expires_at_ms.as_deref(),
            now_ms,
            DEFAULT_REFRESH_SKEW_MS,
        )
    {
        let expires_at_ms = expires_at_ms
            .or_else(|| {
                access_token
                    .as_deref()
                    .and_then(extract_jwt_expires_at_ms)
                    .map(|v| v.to_string())
            })
            .unwrap_or_default();
        set_success_result(
            "LoginComplete",
            &with_cleared_error(json!({
                "expires_at_ms": expires_at_ms,
                "account_id": account_id.unwrap_or_default(),
            })),
        );
        return Ok(());
    }

    refresh_tokens(ctx).map_err(|err| {
        if err.contains("refresh token is missing")
            || err.contains("invalid_grant")
            || err.contains("expired")
        {
            format!("{err}. OpenAI Codex sign-in is required; start the Codex device login again.")
        } else {
            err
        }
    })
}

fn refresh_tokens(ctx: &Context) -> Result<(), String> {
    let auth_base_url = auth_base_url(ctx);
    let refresh_token = resolve_config_or_secret(ctx, SECRET_REFRESH_TOKEN)
        .ok_or("OpenAI Codex refresh token is missing")?;

    let body = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &refresh_token),
        ("client_id", OPENAI_CODEX_CLIENT_ID),
    ]);
    let headers = vec![(
        "content-type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    )];
    let resp = ctx.http_call(
        "POST",
        &format!("{auth_base_url}/oauth/token"),
        &headers,
        &body,
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI Codex token refresh failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }

    let tokens = parse_token_response(&resp.body)?;
    store_tokens_and_complete(ctx, tokens)
}

fn exchange_authorization_code(
    ctx: &Context,
    auth_base_url: &str,
    authorization_code: &str,
    code_verifier: &str,
) -> Result<CodexTokens, String> {
    let body = form_encode(&[
        ("grant_type", "authorization_code"),
        ("code", authorization_code),
        ("redirect_uri", OPENAI_CODEX_DEVICE_CALLBACK_URL),
        ("client_id", OPENAI_CODEX_CLIENT_ID),
        ("code_verifier", code_verifier),
    ]);
    let headers = vec![(
        "content-type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    )];
    let resp = ctx.http_call(
        "POST",
        &format!("{auth_base_url}/oauth/token"),
        &headers,
        &body,
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI device token exchange failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }
    parse_token_response(&resp.body)
}

fn store_tokens_and_complete(ctx: &Context, tokens: CodexTokens) -> Result<(), String> {
    let account_id = extract_chatgpt_account_id_from_jwt(&tokens.access_token)
        .ok_or("OpenAI Codex access token missing chatgpt_account_id")?;
    let expires_at_ms = tokens.expires_at_ms.to_string();

    put_secret(ctx, SECRET_ACCESS_TOKEN, &tokens.access_token)?;
    put_secret(ctx, SECRET_REFRESH_TOKEN, &tokens.refresh_token)?;
    put_secret(ctx, SECRET_EXPIRES_AT_MS, &expires_at_ms)?;
    put_secret(ctx, SECRET_ACCOUNT_ID, &account_id)?;

    set_success_result(
        "LoginComplete",
        &with_cleared_error(json!({
            "expires_at_ms": expires_at_ms,
            "account_id": account_id,
        })),
    );
    Ok(())
}

fn disconnect(ctx: &Context) -> Result<(), String> {
    for key in [
        SECRET_ACCESS_TOKEN,
        SECRET_REFRESH_TOKEN,
        SECRET_EXPIRES_AT_MS,
        SECRET_ACCOUNT_ID,
        "openai_codex_token",
    ] {
        delete_secret(ctx, key)?;
    }
    set_success_result("", &json!({ "status": "disconnected" }));
    Ok(())
}

/// Merge blanked `error`/`error_message` into a success/in-progress result so a
/// prior failure's message does not linger on the entity after auth recovers.
/// `provider_auth_gate` keys off `status`, but the stale error fields misled
/// operators diagnosing Codex auth (2026-06-16: a clean re-login still showed
/// the old `refresh_token_reused` text). Applied to every healthy or
/// in-progress `set_success_result` payload below.
fn with_cleared_error(mut fields: Value) -> Value {
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("error".to_string(), Value::String(String::new()));
        obj.insert("error_message".to_string(), Value::String(String::new()));
    }
    fields
}

fn auth_base_url(ctx: &Context) -> String {
    ctx.config
        .get("openai_auth_base_url")
        .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_OPENAI_AUTH_BASE_URL.to_string())
}

fn resolve_config_or_secret(ctx: &Context, key: &str) -> Option<String> {
    ctx.config
        .get(key)
        .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
        .cloned()
        .or_else(|| ctx.get_secret(key).ok())
}

fn put_secret(ctx: &Context, key: &str, value: &str) -> Result<(), String> {
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let temper_api_url = resolve_temper_api_url(ctx, &fields);
    let headers = runtime_headers(ctx, &ctx.tenant, &fields, Some("application/json"), None);
    let body = json!({ "key": key, "value": value }).to_string();
    let resp = ctx.http_call(
        "POST",
        &format!("{temper_api_url}/paw/setup/secrets"),
        &headers,
        &body,
    )?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "saving Codex secret {key} failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }
    Ok(())
}

fn delete_secret(ctx: &Context, key: &str) -> Result<(), String> {
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let temper_api_url = resolve_temper_api_url(ctx, &fields);
    let headers = runtime_headers(ctx, &ctx.tenant, &fields, Some("application/json"), None);
    let resp = ctx.http_call(
        "DELETE",
        &format!("{temper_api_url}/paw/setup/secrets/{key}"),
        &headers,
        "",
    )?;
    if !(200..300).contains(&resp.status) && resp.status != 404 {
        return Err(format!(
            "deleting Codex secret {key} failed: HTTP {} {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexTokens {
    access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
}

fn parse_token_response(body: &str) -> Result<CodexTokens, String> {
    let parsed: Value = serde_json::from_str(body).map_err(|e| format!("token JSON parse: {e}"))?;
    let access_token = string_field(&parsed, "access_token").ok_or("missing access_token")?;
    let refresh_token = string_field(&parsed, "refresh_token").ok_or("missing refresh_token")?;
    let expires_in_ms = parsed
        .get("expires_in")
        .and_then(Value::as_i64)
        .map(|seconds| seconds.max(0) * 1000)
        .unwrap_or(0);
    Ok(CodexTokens {
        access_token,
        refresh_token,
        expires_at_ms: Context::get_time_millis() as i64 + expires_in_ms,
    })
}

fn field_str<'a>(fields: &'a Value, key: &str) -> Option<&'a str> {
    fields
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn body_snippet(body: &str) -> String {
    body.chars().take(300).collect()
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openai_codex_wire::base64_url_no_pad;

    #[test]
    fn form_encode_escapes_device_callback_values() {
        let encoded = form_encode(&[(
            "redirect_uri",
            "https://auth.openai.com/deviceauth/callback",
        )]);
        assert_eq!(
            encoded,
            "redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"
        );
    }

    #[test]
    fn parse_token_response_extracts_expiry_and_tokens() {
        let parsed =
            parse_token_response(r#"{"access_token":"a","refresh_token":"r","expires_in":10}"#)
                .expect("tokens");
        assert_eq!(parsed.access_token, "a");
        assert_eq!(parsed.refresh_token, "r");
        assert!(parsed.expires_at_ms >= 10_000);
    }

    #[test]
    fn account_id_extraction_matches_codex_jwt_claim() {
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test"}}"#;
        let token = format!("h.{}.s", base64_url_no_pad(payload.as_bytes()));
        assert_eq!(
            extract_chatgpt_account_id_from_jwt(&token).as_deref(),
            Some("acct_test")
        );
    }

    #[test]
    fn with_cleared_error_blanks_error_fields_and_preserves_rest() {
        let out = with_cleared_error(json!({
            "account_id": "acct_test",
            "expires_at_ms": "123",
        }));
        assert_eq!(out["error"], json!(""));
        assert_eq!(out["error_message"], json!(""));
        assert_eq!(out["account_id"], json!("acct_test"));
        assert_eq!(out["expires_at_ms"], json!("123"));
    }
}
