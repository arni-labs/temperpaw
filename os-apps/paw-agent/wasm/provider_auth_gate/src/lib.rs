//! Provider Auth Gate — ensures subscription OAuth is fresh before LLM calls.
//!
//! The gate makes one OpenAICodexAuth move for the auth status it reads:
//! PollDeviceLogin while a device code is out, and the configured refresh
//! otherwise. The auth spec's callbacks do the rest, so the move ends with the
//! login ready or with a device code a person can still enter. When it is not
//! ready, the Session fails with the link, the code and how long it lasts;
//! sending the message again after signing in polls the code and continues.

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

    let status_url = format!("{temper_api_url}/paw/setup/openai-codex/status");
    let resp = ctx.http_call("GET", &status_url, &headers, "")?;
    let current: Value = if (200..300).contains(&resp.status) {
        serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    let action = match auth_move(&current, auth_action) {
        AuthMove::Proceed => {
            let status = codex_auth_status_value(&current).unwrap_or("Ready");
            ctx.log(
                "info",
                &format!(
                    "provider_auth_gate: OpenAICodexAuth status is {status}; skipping {auth_action}"
                ),
            );
            set_success_result(ready_action, &ready_params(&fields, status, ""));
            return Ok(());
        }
        AuthMove::Wait => return Err(sign_in_under_way(&current)),
        AuthMove::Dispatch(action) => action,
    };

    let url = format!("{temper_api_url}{}", auth_endpoint_path(action));
    ctx.log(
        "info",
        &format!("provider_auth_gate: dispatching OpenAICodexAuth.{action}"),
    );
    let resp = ctx.http_call("POST", &url, &headers, "{}")?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI Codex auth {action} failed (HTTP {}): {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }

    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({}));
    if !codex_auth_status_allows_provider_call(&parsed) {
        // The route answers once the move and its callbacks are done, so the
        // time left is measured from now, not from before the request.
        return Err(sign_in_needed(&parsed, Context::get_time_millis()));
    }
    let status = codex_auth_status_value(&parsed).unwrap_or("Ready");
    set_success_result(ready_action, &ready_params(&fields, status, ""));
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
        "PollDeviceLogin" => "/paw/setup/openai-codex/poll",
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

/// The one OpenAICodexAuth move the gate makes for an auth status. What
/// follows the move is the auth spec's declared callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthMove<'a> {
    /// The status allows a provider call already.
    Proceed,
    /// A sign-in step is being started, polled or refreshed right now.
    Wait,
    /// Dispatch this OpenAICodexAuth action.
    Dispatch(&'a str),
}

fn auth_move<'a>(status: &Value, auth_action: &'a str) -> AuthMove<'a> {
    if auth_action == "EnsureFresh" && codex_auth_status_allows_provider_call(status) {
        return AuthMove::Proceed;
    }
    match codex_auth_status_value(status)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        // The person may have entered the code already. A code that ran out
        // is replaced by the poll itself.
        Some("devicecodeready") => AuthMove::Dispatch("PollDeviceLogin"),
        Some("starting" | "polling" | "refreshing") => AuthMove::Wait,
        _ => AuthMove::Dispatch(auth_action),
    }
}

/// What a Session fails with when the auth status after the gate's move does
/// not allow a provider call.
fn sign_in_needed(status: &Value, now_ms: i64) -> String {
    sign_in_prompt(status, now_ms).unwrap_or_else(|| {
        format!(
            "OpenAI Codex auth is not ready (status {}{}); send your message again to try once more.",
            codex_auth_status_value(status).unwrap_or("missing"),
            status_text(status, &["last_error", "error"])
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        )
    })
}

/// The link, the code and the time left, when the status holds a device code
/// that has not run out. A Failed entity can still hold an old login's code,
/// so only a DeviceCodeReady status's code is offered.
fn sign_in_prompt(status: &Value, now_ms: i64) -> Option<String> {
    if !codex_auth_status_value(status)?.eq_ignore_ascii_case("devicecodeready") {
        return None;
    }
    let url = status_text(status, &["verification_url", "verification_uri"])?;
    let code = status_text(status, &["user_code"])?;
    let expires_at_ms: i64 = status.get("expires_at_ms").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str()?.trim().parse().ok())
    })?;
    let left_ms = expires_at_ms - now_ms;
    if left_ms <= 0 {
        return None;
    }
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

fn sign_in_under_way(status: &Value) -> String {
    format!(
        "OpenAI Codex sign-in is under way (auth status {}); send your message again in a minute.",
        codex_auth_status_value(status).unwrap_or("missing")
    )
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
        assert_eq!(
            auth_endpoint_path("PollDeviceLogin"),
            "/paw/setup/openai-codex/poll"
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
        assert_eq!(auth_move(&status, "EnsureFresh"), AuthMove::Proceed);
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
    fn a_device_code_that_is_out_is_polled_whatever_its_time_left() {
        // The person may have entered it; a code that ran out is replaced by
        // the poll's own callback, not by a second move from the gate.
        for expires_in_ms in [10 * 60_000, 10_000, -60_000] {
            for auth_action in ["EnsureFresh", "ForceRefresh"] {
                assert_eq!(
                    auth_move(&device_code("DeviceCodeReady", expires_in_ms), auth_action),
                    AuthMove::Dispatch("PollDeviceLogin"),
                    "{auth_action}, {expires_in_ms} ms left"
                );
            }
        }
    }

    #[test]
    fn a_sign_in_step_under_way_is_left_to_finish() {
        for status in [
            json!({"status": "Starting", "configured": false}),
            json!({"status": "Polling", "configured": false}),
            json!({"status": "Refreshing", "configured": false}),
        ] {
            assert_eq!(
                auth_move(&status, "EnsureFresh"),
                AuthMove::Wait,
                "{status}"
            );
        }
        assert_eq!(
            auth_move(
                &json!({"status": "Refreshing", "configured": true}),
                "ForceRefresh"
            ),
            AuthMove::Wait
        );
    }

    #[test]
    fn any_other_status_gets_the_configured_refresh() {
        for status in [
            json!({"status": "Idle", "configured": false}),
            json!({"status": "Failed", "configured": true, "last_error": "refresh_token_reused"}),
            // A Failed entity can still hold the code of an old login.
            device_code("Failed", -20 * 86_400_000),
            json!({"status": "Disconnected", "configured": false}),
            json!({"configured": false}),
            json!({}),
        ] {
            assert_eq!(
                auth_move(&status, "EnsureFresh"),
                AuthMove::Dispatch("EnsureFresh"),
                "{status}"
            );
        }
        // A provider that reported token_expired asks for a refresh even
        // when the status says Ready.
        assert_eq!(
            auth_move(
                &json!({"status": "Ready", "configured": true}),
                "ForceRefresh"
            ),
            AuthMove::Dispatch("ForceRefresh")
        );
    }

    #[test]
    fn the_sign_in_prompt_gives_the_link_the_code_and_the_time_left() {
        let prompt = sign_in_needed(
            &device_code("DeviceCodeReady", 14 * 60_000 + 30_000),
            NOW_MS,
        );
        assert!(
            prompt.contains("https://auth.openai.com/codex/device"),
            "{prompt}"
        );
        assert!(prompt.contains("ABCD-EFGH"), "{prompt}");
        assert!(prompt.contains("14 minutes"), "{prompt}");
        assert!(prompt.contains("send your message again"), "{prompt}");
        // Nothing handles a `codex auth` command, so the prompt never names one.
        assert!(!prompt.contains("codex auth"), "{prompt}");

        let prompt = sign_in_needed(&device_code("DeviceCodeReady", 90_000), NOW_MS);
        assert!(prompt.contains("within a minute"), "{prompt}");
    }

    #[test]
    fn a_code_that_ran_out_or_belongs_to_an_old_login_is_never_offered() {
        for status in [
            device_code("DeviceCodeReady", -59_000),
            device_code("DeviceCodeReady", 0),
            device_code("Failed", 10 * 60_000),
        ] {
            let failure = sign_in_needed(&status, NOW_MS);
            assert!(!failure.contains("ABCD-EFGH"), "{status}: {failure}");
            assert!(failure.contains("send your message again"), "{failure}");
        }
    }
}
