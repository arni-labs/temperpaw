//! Provider Auth Gate — ensures subscription OAuth is fresh before LLM calls.

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
            if let Some(error) = codex_auth_status_blocks_before_dispatch(&parsed) {
                return Err(error);
            }
        }
    }

    let url = format!("{temper_api_url}{}", auth_endpoint_path(auth_action));

    ctx.log(
        "info",
        &format!("provider_auth_gate: dispatching OpenAICodexAuth.{auth_action}"),
    );
    let resp = ctx.http_call("POST", &url, &headers, "{}")?;
    if !(200..300).contains(&resp.status) {
        return Err(format!(
            "OpenAI Codex auth {auth_action} failed (HTTP {}): {}",
            resp.status,
            body_snippet(&resp.body)
        ));
    }

    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or_else(|_| json!({}));
    let status = codex_auth_status_value(&parsed).unwrap_or("Ready");
    if let Some(error) = codex_auth_status_error(&parsed) {
        return Err(error);
    }

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
        _ => "/paw/setup/openai-codex/ensure-fresh",
    }
}

fn codex_auth_status_value(parsed: &Value) -> Option<&str> {
    parsed
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| !status.trim().is_empty())
}

fn codex_auth_status_error(parsed: &Value) -> Option<String> {
    if codex_auth_status_allows_provider_call(parsed) {
        return None;
    }

    let status = codex_auth_status_value(parsed).unwrap_or("missing");
    let detail = parsed
        .get("last_error")
        .or_else(|| parsed.get("error"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());

    if status.eq_ignore_ascii_case("failed") {
        let error = detail.unwrap_or("OpenAI Codex auth failed");
        return Some(format!("{error}. {}", codex_auth_login_guidance(parsed)));
    }

    if matches!(
        status.to_ascii_lowercase().as_str(),
        "starting" | "devicecodeready" | "polling" | "refreshing" | "disconnected" | "idle"
    ) {
        return Some(format!(
            "OpenAI Codex sign-in is required before calling the provider; auth status is {status}. {}",
            codex_auth_login_guidance(parsed)
        ));
    }

    Some(format!(
        "OpenAI Codex auth returned status {status}; OpenAI Codex sign-in is required. {}",
        codex_auth_login_guidance(parsed)
    ))
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

fn codex_auth_status_blocks_before_dispatch(parsed: &Value) -> Option<String> {
    if codex_auth_status_allows_provider_call(parsed) {
        return None;
    }
    let status = codex_auth_status_value(parsed)?;
    if matches!(
        status.to_ascii_lowercase().as_str(),
        "starting" | "devicecodeready" | "polling" | "refreshing" | "disconnected"
    ) {
        return codex_auth_status_error(parsed);
    }
    None
}

fn codex_auth_login_guidance(parsed: &Value) -> String {
    let verification_url = parsed
        .get("verification_uri")
        .or_else(|| parsed.get("verification_url"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let user_code = parsed
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());

    match (verification_url, user_code) {
        (Some(url), Some(code)) => format!(
            "In Discord, DM the bot `codex auth`, open {url}, enter code {code}, then reply `codex auth done`."
        ),
        (Some(url), None) => format!(
            "In Discord, DM the bot `codex auth`, open {url}, complete sign-in, then reply `codex auth done`."
        ),
        (None, Some(code)) => format!(
            "In Discord, DM the bot `codex auth`, enter code {code} at the device-login page, then reply `codex auth done`."
        ),
        (None, None) => "In Discord, DM the bot `codex auth`, complete the device login, then reply `codex auth done`.".to_string(),
    }
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
        assert!(codex_auth_status_error(&status).is_none());
    }

    #[test]
    fn device_code_codex_auth_blocks_with_discord_guidance() {
        let status = json!({
            "status": "DeviceCodeReady",
            "configured": false,
            "verification_uri": "https://microsoft.com/devicelogin",
            "user_code": "ABCD-EFGH"
        });

        let error = codex_auth_status_error(&status).expect("device code should block");
        assert!(error.contains("DM the bot"));
        assert!(error.contains("ABCD-EFGH"));
        assert!(error.contains("https://microsoft.com/devicelogin"));
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

        assert!(codex_auth_status_blocks_before_dispatch(&status).is_some());
    }
}
