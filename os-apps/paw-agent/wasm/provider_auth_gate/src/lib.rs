//! Provider Auth Gate — ensures subscription OAuth is fresh before LLM calls.
//!
//! When OpenAI Codex needs a person to sign in, the gate polls a device code
//! that is still usable (they may have entered it already) and otherwise
//! starts a fresh device login. The Session then fails with the link, the
//! code and how long it lasts; sending the message again after signing in
//! polls the code and continues.

use temper_wasm_sdk::prelude::*;
use wasm_helpers::{resolve_temper_api_url, runtime_headers_as, timestamp_millis_string};

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    if let Err(err) = run_provider_auth_gate() {
        set_error_result(&err);
    }
    0
}

fn run_provider_auth_gate() -> Result<(), String> {
    let ctx = Context::from_host()?;
    let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
    let provider = selected_provider(&ctx, &fields);
    let ready_action = config_value(&ctx, "ready_action").unwrap_or("ProviderAuthReady");

    if provider != "openai_codex" {
        set_success_result(ready_action, &ready_params(&fields, "skipped", ""));
        return Ok(());
    }

    let auth_action = config_value(&ctx, "auth_action").unwrap_or("EnsureFresh");
    let temper_api_url = resolve_temper_api_url(&ctx, &fields);
    let headers = runtime_headers_as(
        &ctx,
        &ctx.tenant,
        &fields,
        "system",
        Some("application/json"),
        Some("application/json"),
    );

    if auth_action == "EnsureFresh" {
        let status_url = format!("{temper_api_url}/paw/setup/openai-codex/status");
        let resp = ctx.http_call("GET", &status_url, &headers, "")?;
        if (200..300).contains(&resp.status) {
            let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({}));
            let status = codex_auth_status_value(&parsed).unwrap_or("");
            if codex_auth_status_allows_provider_call(&parsed) {
                ctx.log(
                    "info",
                    &format!(
                        "provider_auth_gate: OpenAICodexAuth status is {status}; skipping EnsureFresh"
                    ),
                );
                set_success_result(ready_action, &ready_params(&fields, status, ""));
                return Ok(());
            }
            if codex_auth_status_needs_person(&parsed) {
                let status = sign_in(&ctx, &temper_api_url, &headers, &parsed)?;
                set_success_result(ready_action, &ready_params(&fields, &status, ""));
                return Ok(());
            }
        }
    }

    let url = format!("{temper_api_url}{}", auth_endpoint_path(auth_action));

    ctx.log(
        "info",
        &format!("provider_auth_gate: dispatching OpenAICodexAuth.{auth_action}"),
    );
    let resp = ctx.http_call("POST", &url, &headers, "{}")?;
    let parsed: Value = if (200..300).contains(&resp.status) {
        serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({}))
    } else {
        // The refresh moves are not moves from every state (none leaves
        // DeviceCodeReady), so a refused one can still mean a person has to
        // sign in.
        let refused = format!(
            "OpenAI Codex auth {auth_action} failed (HTTP {}): {}",
            resp.status,
            body_snippet(&resp.body)
        );
        let status_url = format!("{temper_api_url}/paw/setup/openai-codex/status");
        let current = ctx
            .http_call("GET", &status_url, &headers, "")
            .ok()
            .filter(|status| (200..300).contains(&status.status))
            .and_then(|status| serde_json::from_str::<Value>(&status.body).ok());
        match current {
            Some(current) if !codex_auth_status_allows_provider_call(&current) => current,
            _ => return Err(refused),
        }
    };
    let status = if codex_auth_status_allows_provider_call(&parsed) {
        codex_auth_status_value(&parsed)
            .unwrap_or("Ready")
            .to_string()
    } else {
        sign_in(&ctx, &temper_api_url, &headers, &parsed)?
    };

    set_success_result(ready_action, &ready_params(&fields, &status, ""));
    Ok(())
}

fn selected_provider(ctx: &Context, fields: &Value) -> String {
    fields
        .get("provider")
        .or_else(|| fields.get("Provider"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config_value(ctx, "default_llm_provider"))
        .map(normalize_provider)
        .unwrap_or_default()
}

fn normalize_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai-codex" => "openai_codex".to_string(),
        "open_router" => "openrouter".to_string(),
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

fn config_value<'a>(ctx: &'a Context, key: &str) -> Option<&'a str> {
    ctx.config
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty() && !value.contains("{secret:"))
}

fn auth_endpoint_path(auth_action: &str) -> &'static str {
    match auth_action {
        "ForceRefresh" => "/paw/setup/openai-codex/force-refresh",
        "Refresh" => "/paw/setup/openai-codex/refresh",
        _ => "/paw/setup/openai-codex/ensure-fresh",
    }
}

fn codex_auth_status_value(parsed: &Value) -> Option<&str> {
    parsed
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| !status.trim().is_empty())
}

fn codex_auth_status_allows_provider_call(parsed: &Value) -> bool {
    let status = codex_auth_status_value(parsed).unwrap_or("");
    if status.eq_ignore_ascii_case("ready") {
        return true;
    }
    status.eq_ignore_ascii_case("refreshing") && codex_auth_status_is_configured(parsed)
}

fn codex_auth_status_is_configured(parsed: &Value) -> bool {
    parsed
        .get("configured")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether the status waits on a person or on a sign-in step under way, so
/// dispatching a refresh would not help. A Failed or Idle entity may still
/// refresh from its stored tokens, so those dispatch first.
fn codex_auth_status_needs_person(parsed: &Value) -> bool {
    if codex_auth_status_allows_provider_call(parsed) {
        return false;
    }
    codex_auth_status_value(parsed).is_some_and(|status| {
        matches!(
            status.to_ascii_lowercase().as_str(),
            "starting" | "devicecodeready" | "polling" | "refreshing" | "disconnected"
        )
    })
}

/// What the gate does with an auth status that needs a person to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignInStep {
    /// A device code is out and still usable: poll it, since the person may
    /// have entered it already.
    Poll,
    /// A sign-in step is being started, polled or refreshed right now.
    InProgress,
    /// No usable device code: start a device login for a fresh one.
    Start,
}

/// A person needs time to open the link and type the code, so a code with
/// less than this left would run out while they enter it.
const DEVICE_CODE_MIN_LEFT_MS: i64 = 30_000;

fn sign_in_step(status: &Value, now_ms: i64) -> SignInStep {
    match codex_auth_status_value(status)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("devicecodeready") if device_code_ms_left(status, now_ms).is_some() => {
            SignInStep::Poll
        }
        Some("starting" | "polling" | "refreshing") => SignInStep::InProgress,
        _ => SignInStep::Start,
    }
}

/// How long the status's device code stays worth entering, if it has one.
/// A Failed entity can still hold an old login's code, so only a
/// DeviceCodeReady status's code is ever offered.
fn device_code_ms_left(status: &Value, now_ms: i64) -> Option<i64> {
    status_text(status, &["verification_url", "verification_uri"])?;
    status_text(status, &["user_code"])?;
    let expires_at_ms = status.get("expires_at_ms").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str()?.trim().parse().ok())
    })?;
    let left_ms = expires_at_ms - now_ms;
    (left_ms > DEVICE_CODE_MIN_LEFT_MS).then_some(left_ms)
}

fn sign_in_prompt(status: &Value, now_ms: i64) -> Option<String> {
    let left_ms = device_code_ms_left(status, now_ms)?;
    let url = status_text(status, &["verification_url", "verification_uri"])?;
    let code = status_text(status, &["user_code"])?;
    let within = match left_ms / 60_000 {
        0 | 1 => "a minute".to_string(),
        minutes => format!("{minutes} minutes"),
    };
    Some(format!(
        "OpenAI Codex sign-in is required. Open {url} and enter code {code} within {within}, then send your message again."
    ))
}

fn status_text<'a>(status: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| status.get(*key).and_then(Value::as_str))
        .find(|value| !value.trim().is_empty())
}

/// Settles an auth status that does not allow a provider call. Returns the
/// status when the person turns out to have signed in, and otherwise the
/// prompt that tells them how, as the error the Session fails with.
fn sign_in(
    ctx: &Context,
    temper_api_url: &str,
    headers: &[(String, String)],
    status: &Value,
) -> Result<String, String> {
    let now_ms = Context::get_time_millis();
    let pending = match sign_in_step(status, now_ms) {
        SignInStep::Poll => {
            ctx.log(
                "info",
                "provider_auth_gate: polling the OpenAI Codex device code",
            );
            let polled =
                codex_auth_post(ctx, temper_api_url, headers, "/paw/setup/openai-codex/poll")?;
            if codex_auth_status_allows_provider_call(&polled) {
                return Ok(codex_auth_status_value(&polled)
                    .unwrap_or("Ready")
                    .to_string());
            }
            match sign_in_step(&polled, now_ms) {
                SignInStep::Poll => polled,
                SignInStep::InProgress => return Err(sign_in_under_way(&polled)),
                SignInStep::Start => start_device_login(ctx, temper_api_url, headers)?,
            }
        }
        SignInStep::InProgress => return Err(sign_in_under_way(status)),
        SignInStep::Start => start_device_login(ctx, temper_api_url, headers)?,
    };
    Err(sign_in_prompt(&pending, now_ms).unwrap_or_else(|| {
        format!(
            "OpenAI Codex sign-in is required, and starting a device login left auth status {}{}.",
            codex_auth_status_value(&pending).unwrap_or("missing"),
            status_text(&pending, &["last_error", "error"])
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        )
    }))
}

fn start_device_login(
    ctx: &Context,
    temper_api_url: &str,
    headers: &[(String, String)],
) -> Result<Value, String> {
    ctx.log(
        "info",
        "provider_auth_gate: starting an OpenAI Codex device login",
    );
    codex_auth_post(
        ctx,
        temper_api_url,
        headers,
        "/paw/setup/openai-codex/device-login",
    )
}

fn sign_in_under_way(status: &Value) -> String {
    format!(
        "OpenAI Codex sign-in is under way (auth status {}); send your message again in a minute.",
        codex_auth_status_value(status).unwrap_or("missing")
    )
}

/// One setup-API sign-in step. The route waits for the auth module, so its
/// answer is the status after the step.
fn codex_auth_post(
    ctx: &Context,
    temper_api_url: &str,
    headers: &[(String, String)],
    path: &str,
) -> Result<Value, String> {
    let resp = ctx.http_call("POST", &format!("{temper_api_url}{path}"), headers, "{}")?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI Codex sign-in step {path} failed (HTTP {}): {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }
    Ok(serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({})))
}

fn ready_params(fields: &Value, status: &str, error: &str) -> Value {
    ready_params_with_checked_at(fields, status, error, timestamp_millis_string())
}

fn ready_params_with_checked_at(
    fields: &Value,
    status: &str,
    error: &str,
    checked_at_ms: String,
) -> Value {
    json!({
        "provider_auth_status": status,
        "provider_auth_checked_at_ms": checked_at_ms,
        "provider_auth_error": error,
        "provider_auth_retry_count": retry_count(fields, "provider_auth_retry_count", "ProviderAuthRetryCount"),
        "compaction_auth_retry_count": retry_count(fields, "compaction_auth_retry_count", "CompactionAuthRetryCount"),
    })
}

fn retry_count(fields: &Value, snake_case: &str, pascal_case: &str) -> i64 {
    fields
        .get(snake_case)
        .or_else(|| fields.get(pascal_case))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|raw| raw.parse::<i64>().ok()))
        })
        .unwrap_or(0)
}

fn body_snippet(body: &str) -> String {
    body.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_provider_recognizes_codex_aliases() {
        assert_eq!(normalize_provider("codex"), "openai_codex");
        assert_eq!(normalize_provider("openai-codex"), "openai_codex");
        assert_eq!(normalize_provider("open_router"), "openrouter");
    }

    #[test]
    fn auth_endpoint_path_maps_actions_to_setup_triggers() {
        assert_eq!(
            auth_endpoint_path("EnsureFresh"),
            "/paw/setup/openai-codex/ensure-fresh"
        );
        assert_eq!(
            auth_endpoint_path("ForceRefresh"),
            "/paw/setup/openai-codex/force-refresh"
        );
        assert_eq!(
            auth_endpoint_path("Refresh"),
            "/paw/setup/openai-codex/refresh"
        );
    }

    #[test]
    fn ready_params_preserves_existing_retry_counts() {
        let fields = json!({
            "provider_auth_retry_count": 1,
            "compaction_auth_retry_count": "2"
        });
        let params = ready_params_with_checked_at(&fields, "Ready", "", "123".to_string());

        assert_eq!(
            params
                .get("provider_auth_retry_count")
                .and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            params
                .get("compaction_auth_retry_count")
                .and_then(Value::as_i64),
            Some(2)
        );
    }

    #[test]
    fn configured_refreshing_codex_auth_allows_provider_calls() {
        let status = json!({
            "status": "Refreshing",
            "configured": true
        });

        assert!(codex_auth_status_allows_provider_call(&status));
        assert!(!codex_auth_status_needs_person(&status));
    }

    const NOW_MS: i64 = 1_800_000_000_000;

    fn device_code(status: &str, expires_in_ms: i64) -> Value {
        json!({
            "status": status,
            "configured": false,
            "verification_url": "https://auth.openai.com/codex/device",
            "user_code": "ABCD-EFGH",
            "expires_at_ms": (NOW_MS + expires_in_ms).to_string(),
        })
    }

    #[test]
    fn a_usable_device_code_is_polled_before_anyone_is_asked_again() {
        // The person may have entered it already; polling completes the login.
        assert_eq!(
            sign_in_step(&device_code("DeviceCodeReady", 10 * 60_000), NOW_MS),
            SignInStep::Poll
        );
    }

    #[test]
    fn a_sign_in_with_no_usable_code_starts_a_fresh_device_login() {
        for status in [
            device_code("DeviceCodeReady", -60_000),
            device_code("DeviceCodeReady", 10_000),
            json!({"status": "DeviceCodeReady", "configured": false}),
            // A Failed entity can still hold the code of an old login.
            device_code("Failed", -20 * 86_400_000),
            json!({"status": "Failed", "configured": true, "last_error": "refresh_token_reused"}),
            json!({"status": "Idle", "configured": false}),
            json!({"status": "Disconnected", "configured": false}),
            json!({"configured": false}),
        ] {
            assert_eq!(sign_in_step(&status, NOW_MS), SignInStep::Start, "{status}");
        }
    }

    #[test]
    fn a_sign_in_already_under_way_is_left_to_finish() {
        for status in [
            json!({"status": "Starting", "configured": false}),
            json!({"status": "Polling", "configured": false}),
            json!({"status": "Refreshing", "configured": false}),
        ] {
            assert_eq!(
                sign_in_step(&status, NOW_MS),
                SignInStep::InProgress,
                "{status}"
            );
        }
    }

    #[test]
    fn the_sign_in_prompt_gives_the_link_the_code_and_the_time_left() {
        let prompt = sign_in_prompt(
            &device_code("DeviceCodeReady", 14 * 60_000 + 30_000),
            NOW_MS,
        )
        .expect("a usable code makes a prompt");
        assert!(
            prompt.contains("https://auth.openai.com/codex/device"),
            "{prompt}"
        );
        assert!(prompt.contains("ABCD-EFGH"), "{prompt}");
        assert!(prompt.contains("14 minutes"), "{prompt}");
        assert!(prompt.contains("send your message again"), "{prompt}");
        // Nothing handles a `codex auth` command, so the prompt never names one.
        assert!(!prompt.contains("codex auth"), "{prompt}");

        assert_eq!(
            sign_in_prompt(&device_code("DeviceCodeReady", -60_000), NOW_MS),
            None
        );
    }

    #[test]
    fn unconfigured_refreshing_codex_auth_blocks_before_dispatch() {
        let status = json!({
            "status": "Refreshing",
            "configured": false
        });

        assert!(codex_auth_status_needs_person(&status));
    }
}
