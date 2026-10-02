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
        "authentication failed",
        "authentication required",
        "unauthenticated",
        "missing refresh token",
        "expired refresh token",
        "refresh token expired",
        "refresh token has expired",
        "invalid_grant",
        "insufficient_scope",
        "http 401",
        "http 403",
        "api returned 401",
        "api returned 403",
        "\"status\":401",
        "\"status\":403",
        "\"status\": 401",
        "\"status\": 403",
        "invalid credentials",
        "invalid api key",
        "invalid token",
        "token expired",
        "insufficient scope",
        "billing",
        "payment required",
        "insufficient quota",
    ]
    .iter()
    .any(|word| error.contains(word))
    {
        return false;
    }
    [429, 500, 502, 503, 504].iter().any(|status| {
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
fn polling_diagnostic(state: &Value, session: &Value, polls: u64) -> String {
    let program = core::parse(core::field(state, "program_json")).unwrap_or(Value::Null);
    let correction_kind = if core::field(state, "phase") == "compose"
        && program["composition_correction"].is_object()
    {
        "composition_correction"
    } else {
        "response_correction"
    };
    let correction = &program[correction_kind];
    let error: String = core::field(correction, "validation_error")
        .chars()
        .take(1200)
        .collect();
    let fields = session.get("fields").unwrap_or(session);
    format!(
        "Reasoning session remains pending; existing work and original limits are preserved. Session={} status={} polls={} turn_count={} provider_auth_status={} correction_kind={} correction_attempt={} validation_error={}",
        core::field(state, "reasoning_session_id"),
        core::field(session, "Status"),
        polls,
        fields.get("turn_count").unwrap_or(&Value::Null),
        core::field(session, "provider_auth_status"),
        correction_kind,
        correction.get("attempt").unwrap_or(&Value::Null),
        error
    )
}

fn check(ctx: &Context) -> Result<(), String> {
    let started = core::field(&ctx.entity_state, "started_at_ms")
        .parse::<u64>()
        .map_err(|_| "Missing run start time")?;
    if (Context::get_time_millis() as u64).saturating_sub(started) >= core::MAX_MS {
        return Err("Semantic run time budget exhausted; saved work is preserved.".into());
    }
    let polls = ctx.entity_state["counters"]["reasoning_phase_polls"]
        .as_u64()
        .unwrap_or(0);
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
        &format!("{api}/tdata/Sessions('{id}')?$select=Status,result,error_message,error,turn_count,provider_auth_status"),
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
            ctx.log("info", &polling_diagnostic(&ctx.entity_state, &s, polls));
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
    fn pending_diagnostic_identifies_session_state_and_correction_history() {
        let state = json!({"reasoning_session_id":"session-current","program_json":json!({"response_correction":{"attempt":2,"validation_error":"research_evidence[0].url missing"}}).to_string()});
        let message = polling_diagnostic(
            &state,
            &json!({"Status":"CallingProvider","turn_count":1,"provider_auth_status":"ready"}),
            10,
        );
        for expected in [
            "session-current",
            "status=CallingProvider",
            "polls=10",
            "turn_count=1",
            "correction_attempt=2",
            "research_evidence[0].url missing",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }
    #[test]
    fn pending_composition_reports_recorded_graph_rejection() {
        let state = json!({"phase":"compose","reasoning_session_id":"late-composer","program_json":json!({"composition_correction":{"attempt":2,"validation_error":"Reconstructed world omitted or changed a selected causal link"}}).to_string()});
        let message = polling_diagnostic(&state, &json!({"Status":"CallingProvider"}), 10);
        assert!(message.contains("correction_kind=composition_correction"));
        assert!(message.contains("correction_attempt=2"));
        assert!(message.contains("omitted or changed a selected causal link"));
        let mut invalid_json = state.clone();
        invalid_json["program_json"] = json!({"response_correction":{"attempt":1}})
            .to_string()
            .into();
        assert!(
            polling_diagnostic(&invalid_json, &json!({}), 10)
                .contains("correction_kind=response_correction correction_attempt=1")
        );
    }
    #[test]
    fn only_explicit_transient_provider_statuses_retry() {
        for status in [429, 500, 502, 503, 504] {
            assert!(transient_provider_error(&format!(
                "OpenAI Codex API returned {status}: upstream connect error"
            )));
        }
        assert!(transient_provider_error(
            "OpenAI Codex API returned 500: native turn auth context mismatch: scopes"
        ));
        for error in [
            "API returned 401",
            "HTTP 500: authentication failed",
            "HTTP 500: authentication required",
            "HTTP 500: unauthenticated",
            "HTTP 500: missing refresh token",
            "HTTP 500: expired refresh token",
            "HTTP 500: refresh token has expired",
            "HTTP 500: invalid_grant",
            "HTTP 500: insufficient_scope",
            "HTTP 500: upstream HTTP 401",
            "HTTP 500: upstream API returned 403",
            r#"HTTP 500: {"status":403}"#,
            "HTTP 500: invalid credentials",
            "HTTP 500: insufficient scope",
            "HTTP 500: billing limit",
            "HTTP 500: permission denied",
            "HTTP 500: validation failed",
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
