use temper_wasm_sdk::prelude::*;
mod core {
    include!("../../semantic_core.rs");
}
fn transient_provider_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    if [
        "permission",
        "forbidden",
        "unauthorized",
        "denied",
        "validation",
    ]
    .iter()
    .any(|word| error.contains(word))
    {
        return false;
    }
    [429, 502, 503, 504].iter().any(|status| {
        [
            format!("api returned {status}"),
            format!("provider http {status}"),
            format!("http {status}"),
        ]
        .iter()
        .any(|marker| error.contains(marker))
    })
}
fn retry_count(state: &Value) -> u64 {
    state
        .get("counters")
        .and_then(|v| v.get("reasoning_retry_count"))
        .or_else(|| {
            state
                .get("fields")
                .and_then(|v| v.get("reasoning_retry_count"))
        })
        .and_then(Value::as_u64)
        .unwrap_or(0)
}
fn check(ctx: &Context) -> Result<(), String> {
    let polls = ctx.entity_state["counters"]["reasoning_phase_polls"]
        .as_u64()
        .unwrap_or(0);
    if polls > core::MAX_REASONING_POLLS {
        return Err(
            "Reasoning phase exhausted its reserved polling budget; saved work is preserved."
                .into(),
        );
    }
    if core::transition_count(&ctx.entity_state) >= core::MAX_APP_TRANSITIONS {
        return Err("Native transition budget exhausted; saved work is preserved.".into());
    }
    let id = core::field(&ctx.entity_state, "reasoning_session_id");
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("Missing reasoning session".into());
    }
    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|url| !url.is_empty() && !url.contains("{secret:"))
        .ok_or("Missing Temper URL")?;
    let r = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Sessions('{id}')?$select=Status,result,error_message,error"),
        &[
            ("x-tenant-id".into(), ctx.tenant.clone()),
            ("x-temper-principal-kind".into(), "agent".into()),
            ("x-temper-principal-id".into(), ctx.entity_id.clone()),
            ("x-temper-agent-type".into(), "system".into()),
        ],
        "",
    )?;
    if r.status != 200 {
        return Err(format!("Reasoning read HTTP {}", r.status));
    }
    if r.body.len() > 2_000_000 {
        return Err("Reasoning response too large".into());
    }
    let s = core::parse(&r.body)?;
    match core::field(&s, "Status") {
        "Completed" => {
            let result = core::field(&s, "result");
            if result.trim().is_empty() {
                return Err("Reasoning completed without an answer".into());
            }
            set_success_result("ReasoningComplete", &json!({"reasoning_result":result}));
        }
        "Failed" | "Cancelled" => {
            let message = core::field(&s, "error_message");
            let error = if message.trim().is_empty() {
                core::field(&s, "error")
            } else {
                message
            };
            if core::field(&s, "Status") == "Failed"
                && transient_provider_error(error)
                && retry_count(&ctx.entity_state) < 3
            {
                set_success_result(
                    "ReasoningRetry",
                    &json!({"last_retry_error":error,"last_retry_session_id":id}),
                );
                return Ok(());
            }
            return Err(format!("Reasoning session {id} did not complete: {error}"));
        }
        _ => {
            if polls >= core::MAX_REASONING_POLLS {
                return Err("Reasoning phase exhausted its reserved polling budget; saved work is preserved.".into());
            }
            set_success_result("ReasoningPending", &json!({}));
        }
    };
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host().and_then(|ctx| check(&ctx)) {
        Ok(()) => (),
        Err(e) => set_success_result("Fail", &json!({"error_message":e})),
    };
    0
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn only_explicit_transient_provider_statuses_retry() {
        for status in [429, 502, 503, 504] {
            assert!(transient_provider_error(&format!(
                "OpenAI Codex API returned {status}: upstream connect error"
            )));
        }
        for error in [
            "API returned 401",
            "API returned 403",
            "validation error: HTTP 503",
            "permission denied",
            "missing WASM module",
            "connection timed out",
            "invalid JSON",
        ] {
            assert!(!transient_provider_error(error), "{error}");
        }
    }
}
