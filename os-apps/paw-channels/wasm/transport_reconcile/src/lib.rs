use temper_wasm_sdk::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let platform = str_field(&fields, &["platform", "Platform"]).unwrap_or("");
        if platform != "discord" {
            set_success_result(
                "StartFailed",
                &json!({
                    "last_error": format!("unsupported transport platform '{platform}'"),
                }),
            );
            return Ok(());
        }

        let base_url = resolve_temper_api_url(&ctx, &fields);
        let url = format!("{base_url}/paw/internal/transports/discord/start");
        let body = json!({
            "transport_connection_id": ctx.entity_id,
        });
        let headers = runtime_headers(&ctx);

        ctx.log(
            "info",
            &format!("transport_reconcile: starting Discord via {url}"),
        );

        let response = match ctx.http_call("POST", &url, &headers, &body.to_string()) {
            Ok(response) => response,
            Err(error) => {
                emit_retry(&error);
                return Ok(());
            }
        };

        if (200..300).contains(&response.status) {
            let parsed: Value = serde_json::from_str(&response.body).unwrap_or_else(|_| json!({}));
            let interaction_url = parsed
                .get("discord_interaction_url")
                .and_then(Value::as_str)
                .unwrap_or("");
            set_success_result(
                "StartSucceeded",
                &json!({
                    "interaction_url": interaction_url,
                    "last_connected_at": timestamp_now(),
                    "last_error": "",
                    "next_retry_at": "",
                }),
            );
            return Ok(());
        }

        let parsed: Value = serde_json::from_str(&response.body).unwrap_or_else(|_| json!({}));
        let error = parsed
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "Discord transport start returned HTTP {}: {}",
                    response.status, response.body
                )
            });
        let retryable = parsed
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| is_retryable_status(response.status));

        if retryable {
            emit_retry(&error);
        } else {
            set_success_result("StartFailed", &json!({ "last_error": error }));
        }

        Ok(())
    })();

    if let Err(error) = result {
        set_error_result(&error);
    }
    0
}

fn emit_retry(error: &str) {
    set_success_result(
        "StartRetry",
        &json!({
            "last_error": error,
            "next_retry_at": timestamp_after_secs(30),
        }),
    );
}

fn runtime_headers(ctx: &Context) -> Vec<(String, String)> {
    vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("x-tenant-id".to_string(), ctx.tenant.clone()),
        ("x-temper-principal-kind".to_string(), "admin".to_string()),
        (
            "x-temper-principal-id".to_string(),
            "transport-reconcile-wasm".to_string(),
        ),
    ]
}

fn resolve_temper_api_url(ctx: &Context, fields: &Value) -> String {
    fields
        .get("temper_api_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            ctx.config
                .get("temper_api_url")
                .filter(|value| !value.is_empty() && !value.starts_with("{secret:"))
                .cloned()
        })
        .unwrap_or_else(|| "http://127.0.0.1:3467".to_string())
}

fn str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

fn timestamp_now() -> String {
    Context::get_time_millis().to_string()
}

fn timestamp_after_secs(secs: i64) -> String {
    (Context::get_time_millis() + secs * 1000).to_string()
}

#[cfg(test)]
mod tests {
    use super::{is_retryable_status, resolve_temper_api_url};
    use std::collections::BTreeMap;
    use temper_wasm_sdk::context::Context;
    use temper_wasm_sdk::json;

    #[test]
    fn retryability_matches_start_endpoint_contract() {
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(429));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
    }

    #[test]
    fn temper_api_url_ignores_unresolved_secret_placeholders() {
        let ctx = Context {
            config: BTreeMap::from([(
                "temper_api_url".to_string(),
                "{secret:temper_api_url}".to_string(),
            )]),
            trigger_params: json!({}),
            entity_state: json!({}),
            tenant: "default".to_string(),
            entity_type: "TransportConnection".to_string(),
            entity_id: "transport-discord".to_string(),
            trigger_action: "Start".to_string(),
        };

        assert_eq!(
            resolve_temper_api_url(&ctx, &json!({})),
            "http://127.0.0.1:3467"
        );
    }
}
