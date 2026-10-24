//! Cron Compute Next — single WASM module for cron schedule computation.
//!
//! Handles both Activate and Trigger modes:
//! - **activate**: Parses `schedule` field, computes first `next_run_at`,
//!   returns `ActivateComplete { next_run_at }`.
//! - **trigger**: Same schedule parsing, plus template substitution on
//!   `user_message_template` (replacing `{{run_count}}`, `{{last_result}}`,
//!   `{{now}}`), returns
//!   `TriggerComplete { next_run_at, user_message, model, provider }`.
//!   A job with no template keeps its own `user_message`, unchanged. A job
//!   with no `model` or `provider` runs on the tenant's default, which the
//!   Trigger config names as `default_llm_model` and `default_llm_provider`.
//!
//! A job created through OData stores the CSDL's PascalCase property names
//! (`UserMessage`, `Model`) next to the spec's snake_case fields; the trigger
//! reads either.
//!
//! Mode is determined by the `mode` key in integration config.

use std::collections::BTreeMap;

use temper_wasm_sdk::prelude::*;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));
        let mode = ctx.config.get("mode").map(|s| s.as_str()).unwrap_or("activate");

        ctx.log("info", &format!("cron_compute_next: mode={mode}"));

        // Parse schedule → interval → next_run_at
        let schedule = fields
            .get("schedule")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if schedule.is_empty() {
            return Err("No schedule configured on CronJob".to_string());
        }

        let interval_secs = parse_cron_interval(schedule);
        if interval_secs == 0 {
            return Err(format!(
                "Could not parse cron schedule '{schedule}' into an interval"
            ));
        }

        let now_millis = Context::get_time_millis();
        let now_secs = (now_millis / 1000) as u64;
        let next_secs = now_secs + interval_secs;
        let next_run_at = unix_to_iso8601(next_secs);

        ctx.log(
            "info",
            &format!(
                "cron_compute_next: schedule='{}' interval={}s next_run_at={}",
                schedule, interval_secs, next_run_at
            ),
        );

        match mode {
            "trigger" => {
                // TriggerComplete's user_message becomes this run's Session
                // message and the job's own, so a job with no template sends
                // its message unchanged; an empty one would empty every run.
                let template =
                    job_field(&fields, &["user_message_template", "UserMessageTemplate"]);
                let user_message = if template.is_empty() {
                    job_field(&fields, &["user_message", "UserMessage"]).to_string()
                } else {
                    let run_count = fields
                        .get("run_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let last_result = fields
                        .get("last_result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let now_str = unix_to_iso8601(now_secs);

                    template
                        .replace("{{run_count}}", &run_count.to_string())
                        .replace("{{last_result}}", last_result)
                        .replace("{{now}}", &now_str)
                };

                // Like the message, these become the run's Session settings and
                // the job's own, so the first run writes the tenant default
                // onto a job that had none. A Session with no model stops in
                // context_preparer and says why; failing here instead would
                // stop the job's schedule.
                let model = session_setting(
                    &fields,
                    &["model", "Model"],
                    &ctx.config,
                    "default_llm_model",
                );
                let provider = session_setting(
                    &fields,
                    &["provider", "Provider"],
                    &ctx.config,
                    "default_llm_provider",
                );

                ctx.log(
                    "info",
                    &format!(
                        "cron_compute_next: user_message length={} model='{}' provider='{}'",
                        user_message.len(),
                        model,
                        provider
                    ),
                );

                set_success_result(
                    "TriggerComplete",
                    &json!({
                        "next_run_at": next_run_at,
                        "user_message": user_message,
                        "model": model,
                        "provider": provider,
                    }),
                );
            }
            _ => {
                // activate mode (default)
                set_success_result(
                    "ActivateComplete",
                    &json!({ "next_run_at": next_run_at }),
                );
            }
        }

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

/// The first of `keys` the job holds a non-blank string under, as written,
/// or "".
fn job_field<'a>(fields: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .filter_map(|key| fields.get(*key).and_then(Value::as_str))
        .find(|value| !value.trim().is_empty())
        .unwrap_or("")
}

/// The job's own setting, else the tenant default in `config[default_key]`.
/// The server leaves a `{secret:...}` template in place when the tenant holds
/// no such secret, so a value still holding one is no setting.
fn session_setting(
    fields: &Value,
    keys: &[&str],
    config: &BTreeMap<String, String>,
    default_key: &str,
) -> String {
    let own = job_field(fields, keys).trim();
    let setting = if own.is_empty() {
        config
            .get(default_key)
            .map(|value| value.trim())
            .unwrap_or("")
    } else {
        own
    };
    if setting.contains("{secret:") {
        String::new()
    } else {
        setting.to_string()
    }
}

/// Simple cron interval parser — extracts a repeat interval in seconds.
///
/// Supports common patterns:
/// - `* * * * *` → every 60s (every minute)
/// - `*/5 * * * *` → every 300s (every 5 minutes)
/// - `0 * * * *` → every 3600s (every hour)
/// - `0 */2 * * *` → every 7200s (every 2 hours)
/// - `0 */6 * * *` → every 21600s (every 6 hours)
/// - `0 0 * * *` → every 86400s (every day)
///
/// Returns 0 if the expression can't be parsed into a simple interval.
fn parse_cron_interval(schedule: &str) -> u64 {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return 0;
    }

    let (minute, hour) = (parts[0], parts[1]);

    // Every N minutes: */N * * * *
    if let Some(n) = minute.strip_prefix("*/") {
        if let Ok(n) = n.parse::<u64>() {
            return n * 60;
        }
    }

    // Every minute: * * * * *
    if minute == "*" && hour == "*" {
        return 60;
    }

    // Fixed minute, variable hour patterns
    if minute == "0" {
        // Every N hours: 0 */N * * *
        if let Some(n) = hour.strip_prefix("*/") {
            if let Ok(n) = n.parse::<u64>() {
                return n * 3600;
            }
        }
        // Every hour: 0 * * * *
        if hour == "*" {
            return 3600;
        }
        // Every day: 0 0 * * *
        if hour == "0" {
            return 86400;
        }
    }

    0
}

/// Convert unix seconds to ISO 8601 UTC string.
fn unix_to_iso8601(secs: u64) -> String {
    let mut days = (secs / 86400) as i64;
    let day_secs = secs % 86400;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    let mut year = 1970i64;
    loop {
        let ydays = if is_leap_year(year) { 366 } else { 365 };
        if days < ydays {
            break;
        }
        days -= ydays;
        year += 1;
    }

    let leap = is_leap_year(year);
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 0usize;
    for (i, &md) in mdays.iter().enumerate() {
        if days < md as i64 {
            month = i;
            break;
        }
        days -= md as i64;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month + 1,
        days + 1,
        hour,
        minute,
        second
    )
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
