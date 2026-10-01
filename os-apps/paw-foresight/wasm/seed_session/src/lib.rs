use temper_wasm_sdk::prelude::*;
fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value
        .get("fields")
        .unwrap_or(value)
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
}
fn decision(status: &str, attempt: u64, error: &str) -> (&'static str, String) {
    match status {
        "Completed" if attempt < 3 => ("ResearchIncomplete", "Researcher stopped before saving its completion; continuing saved work.".into()),
        "Completed" => ("ResearchFailed", "Research stopped before SeedComplete after three attempts. Saved findings remain available.".into()),
        "Failed" | "Cancelled" => ("ResearchFailed", if error.is_empty() { format!("Research session {status}; saved findings remain available.") } else { error.chars().take(2000).collect() }),
        _ => ("ResearchPending", String::new()),
    }
}
fn awaiting_current_session(recorded: &str, attempt: u64) -> bool {
    !recorded.is_empty() && recorded.parse::<u64>().ok() != Some(attempt)
}
fn check(ctx: &Context) -> Result<(), String> {
    let id = field(&ctx.entity_state, "research_session_id");
    let attempt = ctx.entity_state["counters"]["research_attempt"]
        .as_u64()
        .ok_or("Missing research attempt")?;
    if awaiting_current_session(
        field(&ctx.entity_state, "research_session_attempt"),
        attempt,
    ) {
        set_success_result(
            "ResearchPending",
            &json!({"expected_research_session_id":id,"expected_research_attempt":attempt,"error_message":""}),
        );
        return Ok(());
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err("Research session identity is missing or invalid".into());
    }
    let api = ctx
        .config
        .get("temper_api_url")
        .filter(|s| !s.is_empty() && !s.contains("{secret:"))
        .ok_or("Missing Temper URL")?;
    let response = ctx.http_call(
        "GET",
        &format!("{api}/tdata/Sessions('{id}')?$select=Status,error_message,error"),
        &[
            ("x-tenant-id".into(), ctx.tenant.clone()),
            ("x-temper-principal-kind".into(), "agent".into()),
            ("x-temper-principal-id".into(), ctx.entity_id.clone()),
            ("x-temper-agent-type".into(), "system".into()),
        ],
        "",
    )?;
    if response.status != 200 {
        return Err(format!("Research session read HTTP {}", response.status));
    }
    if response.body.len() > 64 * 1024 {
        return Err("Research status response exceeds bound".into());
    }
    let session: Value =
        serde_json::from_str(&response.body).map_err(|_| "Invalid research status JSON")?;
    let status = field(&session, "Status");
    if status.is_empty() {
        return Err("Missing research session status".into());
    }
    let error = [field(&session, "error_message"), field(&session, "error")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let (action, mut error) = decision(status, attempt, error);
    if status == "Completed" {
        error = format!("{error} Original terminal reply remains in Session {id}.");
    }
    set_success_result(
        action,
        &json!({"expected_research_session_id":id,"expected_research_attempt":attempt,"research_session_attempt":attempt.to_string(),"error_message":error}),
    );
    Ok(())
}
#[unsafe(no_mangle)]
pub extern "C" fn run(_: i32, _: i32) -> i32 {
    match Context::from_host() {
        Ok(ctx) => {
            if let Err(error) = check(&ctx) {
                set_success_result(
                    "ResearchMonitorUnavailable",
                    &json!({"expected_research_session_id":field(&ctx.entity_state,"research_session_id"),"expected_research_attempt":ctx.entity_state["counters"]["research_attempt"],"error_message":error}),
                );
            }
        }
        Err(error) => set_error_result(&error),
    }
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_completed_session_cannot_restart_a_new_attempt_during_spawn() {
        assert!(awaiting_current_session("1", 2));
        assert!(!awaiting_current_session("2", 2));
        assert!(!awaiting_current_session("", 1)); // Existing live worlds predate this field.
    }
    #[test]
    fn terminal_without_application_completion_retries_boundedly() {
        assert_eq!(decision("Completed", 1, "").0, "ResearchIncomplete");
        assert_eq!(decision("Completed", 2, "").0, "ResearchIncomplete");
        assert_eq!(decision("Completed", 3, "").0, "ResearchFailed");
        assert_eq!(decision("Executing", 1, "").0, "ResearchPending");
    }
    #[test]
    fn auth_failure_surfaces_exact_reason_without_retry() {
        let reason = "OpenAI Codex refresh token missing; device login required";
        assert_eq!(
            decision("Failed", 1, reason),
            ("ResearchFailed", reason.into())
        );
    }
}
